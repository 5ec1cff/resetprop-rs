use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::offset_of;

use crate::prop_info::{
    align_up, PropertyInfo, RawLongProperty, RawPropAreaHeader, RawPropInfoHeader,
    RawTrieNodeHeader, INITIAL_BYTES_USED, LONG_LEGACY_ERROR, LONG_OFFSET_IN_INFO,
    PROP_AREA_HEADER_SIZE, PROP_AREA_MAGIC, PROP_AREA_VERSION, PROP_INFO_LONG_FLAG, PROP_INFO_SIZE,
    PROP_TRIE_NODE_HEADER_SIZE, PROP_VALUE_MAX,
};

// ── Offset constants derived entirely from the repr(C) structs ────────────────

const BYTES_USED_OFFSET: u64 = offset_of!(RawPropAreaHeader, bytes_used) as u64;
const MAGIC_OFFSET:      u64 = offset_of!(RawPropAreaHeader, magic)      as u64;
const VERSION_OFFSET:    u64 = offset_of!(RawPropAreaHeader, version)    as u64;

const NODE_PROP_OFFSET:     u32 = offset_of!(RawTrieNodeHeader, prop)     as u32;
const NODE_LEFT_OFFSET:     u32 = offset_of!(RawTrieNodeHeader, left)     as u32;
const NODE_RIGHT_OFFSET:    u32 = offset_of!(RawTrieNodeHeader, right)    as u32;
const NODE_CHILDREN_OFFSET: u32 = offset_of!(RawTrieNodeHeader, children) as u32;

const PROP_SERIAL_OFFSET: u32 = offset_of!(RawPropInfoHeader, serial) as u32;

pub type Result<T> = std::result::Result<T, PropAreaError>;

#[derive(Debug)]
pub enum PropAreaError {
    Io(io::Error),
    Utf8(std::string::FromUtf8Error),
    AreaTooSmall(u64),
    AreaTooLarge(u64),
    InvalidMagic(u32),
    InvalidVersion(u32),
    InvalidBytesUsed(u32),
    InvalidOffset(u32),
    InvalidKey(String),
    InPlaceUpdateTooLong {
        name: String,
        new_len: usize,
        max_len: usize,
    },
    Corrupted(&'static str),
    AreaFull { requested: u32, available: u32 },
}

impl fmt::Display for PropAreaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Utf8(err) => write!(f, "utf8 error: {err}"),
            Self::AreaTooSmall(size) => write!(f, "prop area too small: {size} bytes"),
            Self::AreaTooLarge(size) => write!(f, "prop area too large: {size} bytes"),
            Self::InvalidMagic(magic) => write!(f, "invalid prop area magic: 0x{magic:08x}"),
            Self::InvalidVersion(version) => {
                write!(f, "invalid prop area version: 0x{version:08x}")
            }
            Self::InvalidBytesUsed(bytes_used) => {
                write!(f, "invalid bytes_used value: {bytes_used}")
            }
            Self::InvalidOffset(offset) => write!(f, "invalid data offset: {offset}"),
            Self::InvalidKey(key) => write!(f, "invalid property key: {key}"),
            Self::InPlaceUpdateTooLong {
                name,
                new_len,
                max_len,
            } => write!(
                f,
                "cannot update property '{name}' in place: new value length {new_len} exceeds max {max_len}"
            ),
            Self::Corrupted(message) => write!(f, "corrupted prop area: {message}"),
            Self::AreaFull {
                requested,
                available,
            } => write!(
                f,
                "prop area is full: requested {requested} bytes, only {available} bytes available"
            ),
        }
    }
}

impl std::error::Error for PropAreaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Utf8(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for PropAreaError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<std::string::FromUtf8Error> for PropAreaError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        Self::Utf8(value)
    }
}

#[derive(Debug, Clone)]
struct TrieNodeRecord {
    offset: u32,
    namelen: u32,
    prop: u32,
    left: u32,
    right: u32,
    children: u32,
    name: String,
}

#[derive(Debug, Clone)]
struct PropRecord {
    name: String,
    value: String,
    prop_offset: u32,
    value_offset: u32,
    is_long: bool,
}

pub struct PropArea<M> {
    inner: M,
    area_size: u64,
    data_size: u32,
}

impl<M: Read + Seek> PropArea<M> {
    pub fn new(inner: M) -> Result<Self> {
        let mut inner = inner;
        let area_size = inner.seek(SeekFrom::End(0))?;
        Self::from_inner_with_size(inner, area_size)
    }

    pub fn area_size(&self) -> u64 {
        self.area_size
    }

    pub fn data_size(&self) -> u32 {
        self.data_size
    }

    pub fn into_inner(self) -> M {
        self.inner
    }

    pub fn get_property(&mut self, key: &str) -> Result<Option<String>> {
        Ok(self
            .get_property_info(key)?
            .map(|property_info| property_info.value))
    }

    pub fn get_property_info(&mut self, key: &str) -> Result<Option<PropertyInfo>> {
        let node_offset = match self.traverse_trie(key)? {
            Some(offset) => offset,
            None => return Ok(None),
        };

        let prop_offset = self.read_u32_data(node_offset + NODE_PROP_OFFSET)?;
        if prop_offset == 0 {
            return Ok(None);
        }

        let record = self.read_prop_record(prop_offset)?;
        Ok(Some(self.to_property_info(record)))
    }

    pub fn for_each_property<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(PropertyInfo),
    {
        let mut visited = BTreeSet::new();
        self.for_each_from(0, &mut visited, &mut callback)
    }

    pub fn for_each_property_info<F>(&mut self, callback: F) -> Result<()>
    where
        F: FnMut(PropertyInfo),
    {
        self.for_each_property(callback)
    }

    fn from_inner_with_size(inner: M, area_size: u64) -> Result<Self> {
        if area_size < PROP_AREA_HEADER_SIZE {
            return Err(PropAreaError::AreaTooSmall(area_size));
        }

        let data_size = area_size
            .checked_sub(PROP_AREA_HEADER_SIZE)
            .ok_or(PropAreaError::AreaTooSmall(area_size))?;
        let data_size = u32::try_from(data_size).map_err(|_| PropAreaError::AreaTooLarge(area_size))?;

        let mut this = Self {
            inner,
            area_size,
            data_size,
        };

        let magic = this.read_u32_abs(MAGIC_OFFSET)?;
        if magic != PROP_AREA_MAGIC {
            return Err(PropAreaError::InvalidMagic(magic));
        }

        let version = this.read_u32_abs(VERSION_OFFSET)?;
        if version != PROP_AREA_VERSION {
            return Err(PropAreaError::InvalidVersion(version));
        }

        let bytes_used = this.bytes_used()?;
        if bytes_used < PROP_TRIE_NODE_HEADER_SIZE || bytes_used > this.data_size {
            return Err(PropAreaError::InvalidBytesUsed(bytes_used));
        }

        Ok(this)
    }

    fn bytes_used(&mut self) -> Result<u32> {
        self.read_u32_abs(BYTES_USED_OFFSET)
    }

    fn validate_key<'a>(&self, key: &'a str) -> Result<Vec<&'a str>> {
        if key.is_empty() {
            return Err(PropAreaError::InvalidKey(key.to_owned()));
        }

        let segments: Vec<_> = key.split('.').collect();
        if segments.iter().any(|segment| segment.is_empty()) {
            return Err(PropAreaError::InvalidKey(key.to_owned()));
        }

        Ok(segments)
    }

    fn traverse_trie(&mut self, key: &str) -> Result<Option<u32>> {
        let segments = self.validate_key(key)?;
        let mut current_offset = 0u32;

        for segment in segments {
            let current = self.read_node(current_offset)?;
            if current.children == 0 {
                return Ok(None);
            }

            let next = self.find_sibling(current.children, segment)?;
            let Some(next_offset) = next else {
                return Ok(None);
            };
            current_offset = next_offset;
        }

        Ok(Some(current_offset))
    }

    fn find_sibling(&mut self, root_offset: u32, target: &str) -> Result<Option<u32>> {
        let mut current_offset = root_offset;
        let max_steps = usize::max(1, self.data_size as usize / PROP_TRIE_NODE_HEADER_SIZE as usize);

        for _ in 0..max_steps {
            let current = self.read_node(current_offset)?;
            match cmp_prop_name(target, &current.name) {
                std::cmp::Ordering::Equal => return Ok(Some(current_offset)),
                std::cmp::Ordering::Less => {
                    if current.left == 0 {
                        return Ok(None);
                    }
                    current_offset = current.left;
                }
                std::cmp::Ordering::Greater => {
                    if current.right == 0 {
                        return Ok(None);
                    }
                    current_offset = current.right;
                }
            }
        }

        Err(PropAreaError::Corrupted("possible cycle in sibling tree"))
    }

    fn read_node(&mut self, offset: u32) -> Result<TrieNodeRecord> {
        let header = self.read_data(offset, PROP_TRIE_NODE_HEADER_SIZE)?;
        let namelen  = read_u32_at(&header, offset_of!(RawTrieNodeHeader, namelen));
        let prop     = read_u32_at(&header, offset_of!(RawTrieNodeHeader, prop));
        let left     = read_u32_at(&header, offset_of!(RawTrieNodeHeader, left));
        let right    = read_u32_at(&header, offset_of!(RawTrieNodeHeader, right));
        let children = read_u32_at(&header, offset_of!(RawTrieNodeHeader, children));

        let name = if namelen == 0 {
            String::new()
        } else {
            self.read_c_string(offset + PROP_TRIE_NODE_HEADER_SIZE, Some(namelen + 1))?
        };

        Ok(TrieNodeRecord {
            offset,
            namelen,
            prop,
            left,
            right,
            children,
            name,
        })
    }

    fn read_prop_record(&mut self, prop_offset: u32) -> Result<PropRecord> {
        let header = self.read_data(prop_offset, PROP_INFO_SIZE)?;
        let name = self.read_c_string(prop_offset + PROP_INFO_SIZE, None)?;
        let is_long = self.is_long_prop(&header);

        let value_offset = if is_long {
            let long_offset = read_u32_at(&header,
                offset_of!(RawPropInfoHeader, value) + offset_of!(RawLongProperty, offset),
            );
            let min_long_offset = align_up(PROP_INFO_SIZE + name.len() as u32 + 1, 4);
            if long_offset < min_long_offset {
                return Err(PropAreaError::Corrupted("long property offset points into prop_info"));
            }

            prop_offset
                .checked_add(long_offset)
                .ok_or(PropAreaError::InvalidOffset(prop_offset))?
        } else {
            prop_offset + 4
        };

        let value = if is_long {
            self.read_c_string(value_offset, None)?
        } else {
            let raw_value = &header[4..4 + PROP_VALUE_MAX];
            let end = raw_value
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(PropAreaError::Corrupted("inline property value is not null terminated"))?;
            String::from_utf8(raw_value[..end].to_vec())?
        };

        Ok(PropRecord {
            name,
            value,
            prop_offset,
            value_offset,
            is_long,
        })
    }

    fn is_long_prop(&self, header: &[u8]) -> bool {
        let serial = read_u32_at(header, offset_of!(RawPropInfoHeader, serial));
        (serial & PROP_INFO_LONG_FLAG) != 0
    }

    fn for_each_from<F>(
        &mut self,
        offset: u32,
        visited: &mut BTreeSet<u32>,
        callback: &mut F,
    ) -> Result<()>
    where
        F: FnMut(PropertyInfo),
    {
        if !visited.insert(offset) {
            return Err(PropAreaError::Corrupted("cycle detected while iterating trie"));
        }

        let node = self.read_node(offset)?;
        if node.left != 0 {
            self.for_each_from(node.left, visited, callback)?;
        }

        if node.prop != 0 {
            let record = self.read_prop_record(node.prop)?;
            callback(self.to_property_info(record));
        }

        if node.children != 0 {
            self.for_each_from(node.children, visited, callback)?;
        }

        if node.right != 0 {
            self.for_each_from(node.right, visited, callback)?;
        }

        visited.remove(&offset);
        Ok(())
    }

    fn to_property_info(&self, record: PropRecord) -> PropertyInfo {
        PropertyInfo {
            name: record.name,
            value: record.value,
            prop_offset: record.prop_offset,
            name_offset: record.prop_offset + PROP_INFO_SIZE,
            value_offset: record.value_offset,
            is_long: record.is_long,
        }
    }

    fn read_u32_abs(&mut self, absolute_offset: u64) -> Result<u32> {
        let mut bytes = [0u8; 4];
        self.inner.seek(SeekFrom::Start(absolute_offset))?;
        self.inner.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_u32_data(&mut self, data_offset: u32) -> Result<u32> {
        let absolute_offset = self.check_range(data_offset, 4)?;
        self.read_u32_abs(absolute_offset)
    }

    fn read_data(&mut self, data_offset: u32, len: u32) -> Result<Vec<u8>> {
        let absolute_offset = self.check_range(data_offset, len)?;
        let mut buffer = vec![0u8; len as usize];
        self.inner.seek(SeekFrom::Start(absolute_offset))?;
        self.inner.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn read_c_string(&mut self, data_offset: u32, max_len: Option<u32>) -> Result<String> {
        let bytes = self.read_c_string_bytes(data_offset, max_len)?;
        Ok(String::from_utf8(bytes)?)
    }

    fn read_c_string_bytes(&mut self, data_offset: u32, max_len: Option<u32>) -> Result<Vec<u8>> {
        let limit = max_len.unwrap_or(self.data_size.saturating_sub(data_offset));
        if data_offset > self.data_size {
            return Err(PropAreaError::InvalidOffset(data_offset));
        }

        let mut bytes = Vec::new();
        for idx in 0..limit {
            let absolute_offset = self.check_range(data_offset + idx, 1)?;
            self.inner.seek(SeekFrom::Start(absolute_offset))?;
            let mut byte = [0u8; 1];
            self.inner.read_exact(&mut byte)?;
            if byte[0] == 0 {
                return Ok(bytes);
            }
            bytes.push(byte[0]);
        }

        Err(PropAreaError::Corrupted("unterminated c string"))
    }

    fn check_range(&self, data_offset: u32, len: u32) -> Result<u64> {
        let end = data_offset
            .checked_add(len)
            .ok_or(PropAreaError::InvalidOffset(data_offset))?;
        if end > self.data_size {
            return Err(PropAreaError::InvalidOffset(data_offset));
        }
        Ok(PROP_AREA_HEADER_SIZE + data_offset as u64)
    }
}

impl<M: Read + Write + Seek> PropArea<M> {
    pub fn create(mut inner: M, area_size: u64) -> Result<Self> {
        if area_size < PROP_AREA_HEADER_SIZE + PROP_TRIE_NODE_HEADER_SIZE as u64 {
            return Err(PropAreaError::AreaTooSmall(area_size));
        }

        zero_fill(&mut inner, area_size)?;
        inner.seek(SeekFrom::Start(BYTES_USED_OFFSET))?;
        inner.write_all(&INITIAL_BYTES_USED.to_le_bytes())?;
        inner.write_all(&0u32.to_le_bytes())?;
        inner.write_all(&PROP_AREA_MAGIC.to_le_bytes())?;
        inner.write_all(&PROP_AREA_VERSION.to_le_bytes())?;
        inner.flush()?;

        Self::from_inner_with_size(inner, area_size)
    }

    pub fn set_property(&mut self, key: &str, value: &str) -> Result<()> {
        let node_offset = self.ensure_traverse_trie(key)?;
        let prop_offset = self.read_u32_data(node_offset + NODE_PROP_OFFSET)?;

        if prop_offset == 0 {
            let new_prop_offset = self.create_prop_info(key, value)?;
            self.write_u32_data(node_offset + NODE_PROP_OFFSET, new_prop_offset)?;
            return Ok(());
        }

        let record = self.read_prop_record(prop_offset)?;
        if record.is_long {
            self.update_long_property(prop_offset, &record.name, value)?;
        } else {
            self.update_inline_property_in_place(prop_offset, &record.name, value)?;
        }

        Ok(())
    }

    pub fn delete_property(&mut self, key: &str) -> Result<bool> {
        let Some(node_offset) = self.traverse_trie(key)? else {
            return Ok(false);
        };

        let prop_offset = self.read_u32_data(node_offset + NODE_PROP_OFFSET)?;
        if prop_offset == 0 {
            return Ok(false);
        }

        self.write_u32_data(node_offset + NODE_PROP_OFFSET, 0)?;
        self.wipe_prop_info(prop_offset)?;
        let _ = self.prune_trie(0)?;
        Ok(true)
    }

    fn ensure_traverse_trie(&mut self, key: &str) -> Result<u32> {
        let segments = self.validate_key(key)?;
        let mut current_offset = 0u32;

        for segment in segments {
            let current = self.read_node(current_offset)?;
            let child_root = if current.children != 0 {
                current.children
            } else {
                let new_offset = self.create_trie_node(segment)?;
                self.write_u32_data(current.offset + NODE_CHILDREN_OFFSET, new_offset)?;
                new_offset
            };

            current_offset = self.ensure_sibling(child_root, segment)?;
        }

        Ok(current_offset)
    }

    fn ensure_sibling(&mut self, root_offset: u32, target: &str) -> Result<u32> {
        let mut current_offset = root_offset;
        let max_steps = usize::max(1, self.data_size as usize / PROP_TRIE_NODE_HEADER_SIZE as usize);

        for _ in 0..max_steps {
            let current = self.read_node(current_offset)?;
            match cmp_prop_name(target, &current.name) {
                std::cmp::Ordering::Equal => return Ok(current_offset),
                std::cmp::Ordering::Less => {
                    if current.left != 0 {
                        current_offset = current.left;
                    } else {
                        let new_offset = self.create_trie_node(target)?;
                        self.write_u32_data(current.offset + NODE_LEFT_OFFSET, new_offset)?;
                        return Ok(new_offset);
                    }
                }
                std::cmp::Ordering::Greater => {
                    if current.right != 0 {
                        current_offset = current.right;
                    } else {
                        let new_offset = self.create_trie_node(target)?;
                        self.write_u32_data(current.offset + NODE_RIGHT_OFFSET, new_offset)?;
                        return Ok(new_offset);
                    }
                }
            }
        }

        Err(PropAreaError::Corrupted("possible cycle while inserting sibling node"))
    }

    fn create_trie_node(&mut self, name: &str) -> Result<u32> {
        let name_len = u32::try_from(name.len()).map_err(|_| PropAreaError::Corrupted("name too long"))?;
        let node_size = PROP_TRIE_NODE_HEADER_SIZE + name_len + 1;
        let offset = self.allocate_obj(node_size)?;

        self.write_u32_data(offset, name_len)?;
        self.write_u32_data(offset + NODE_PROP_OFFSET, 0)?;
        self.write_u32_data(offset + NODE_LEFT_OFFSET, 0)?;
        self.write_u32_data(offset + NODE_RIGHT_OFFSET, 0)?;
        self.write_u32_data(offset + NODE_CHILDREN_OFFSET, 0)?;
        self.write_bytes_data(offset + PROP_TRIE_NODE_HEADER_SIZE, name.as_bytes())?;
        self.write_bytes_data(offset + PROP_TRIE_NODE_HEADER_SIZE + name_len, &[0])?;

        Ok(offset)
    }

    fn create_prop_info(&mut self, name: &str, value: &str) -> Result<u32> {
        let name_len = u32::try_from(name.len()).map_err(|_| PropAreaError::Corrupted("name too long"))?;
        let prop_offset = self.allocate_obj(PROP_INFO_SIZE + name_len + 1)?;

        self.write_u32_data(prop_offset, 0)?;
        if value.len() >= PROP_VALUE_MAX {
            self.write_long_layout(prop_offset, name_len, value)?;
        } else {
            self.initialize_inline_property(prop_offset, value)?;
        }

        self.write_bytes_data(prop_offset + PROP_INFO_SIZE, name.as_bytes())?;
        self.write_bytes_data(prop_offset + PROP_INFO_SIZE + name_len, &[0])?;
        Ok(prop_offset)
    }

    fn write_inline_value_bytes(&mut self, prop_offset: u32, value: &str) -> Result<()> {
        if value.len() >= PROP_VALUE_MAX {
            return Err(PropAreaError::Corrupted("inline property value too large"));
        }

        self.zero_data(prop_offset + 4, PROP_VALUE_MAX as u32)?;
        self.write_bytes_data(prop_offset + 4, value.as_bytes())?;
        self.write_bytes_data(prop_offset + 4 + value.len() as u32, &[0])?;
        Ok(())
    }

    fn initialize_inline_property(&mut self, prop_offset: u32, value: &str) -> Result<()> {
        if value.len() >= PROP_VALUE_MAX {
            return Err(PropAreaError::Corrupted("inline property value too large"));
        }

        self.write_inline_value_bytes(prop_offset, value)?;
        let serial = (value.len() as u32) << 24;
        self.write_u32_data(prop_offset + PROP_SERIAL_OFFSET, serial)?;
        Ok(())
    }

    fn update_inline_property_in_place(&mut self, prop_offset: u32, name: &str, value: &str) -> Result<()> {
        if value.len() >= PROP_VALUE_MAX {
            return Err(PropAreaError::InPlaceUpdateTooLong {
                name: name.to_owned(),
                new_len: value.len(),
                max_len: PROP_VALUE_MAX - 1,
            });
        }

        self.write_inline_value_bytes(prop_offset, value)
    }

    fn update_long_property(&mut self, prop_offset: u32, name: &str, value: &str) -> Result<()> {
        let header = self.read_data(prop_offset, PROP_INFO_SIZE)?;
        let current_rel = read_u32_at(
            &header,
            offset_of!(RawPropInfoHeader, value) + offset_of!(RawLongProperty, offset),
        );
        let current_offset = prop_offset
            .checked_add(current_rel)
            .ok_or(PropAreaError::InvalidOffset(prop_offset))?;
        let current_bytes = self.read_c_string_bytes(current_offset, None)?;
        let current_len = current_bytes.len();
        if value.len() > current_len {
            return Err(PropAreaError::InPlaceUpdateTooLong {
                name: name.to_owned(),
                new_len: value.len(),
                max_len: current_len,
            });
        }
        let current_capacity = current_len as u32 + 1;

        self.zero_data(current_offset, current_capacity)?;
        self.write_bytes_data(current_offset, value.as_bytes())?;
        self.write_bytes_data(current_offset + value.len() as u32, &[0])?;
        Ok(())
    }

    fn write_long_layout(&mut self, prop_offset: u32, name_len: u32, value: &str) -> Result<()> {
        let long_value_size = u32::try_from(value.len() + 1)
            .map_err(|_| PropAreaError::Corrupted("value too large to store"))?;
        let long_value_offset = self.allocate_obj(long_value_size)?;
        let relative_offset = long_value_offset
            .checked_sub(prop_offset)
            .ok_or(PropAreaError::InvalidOffset(long_value_offset))?;
        let min_expected = align_up(PROP_INFO_SIZE + name_len + 1, 4);
        if relative_offset < min_expected {
            return Err(PropAreaError::Corrupted("invalid long value placement"));
        }

        self.zero_data(prop_offset + 4, PROP_VALUE_MAX as u32)?;
        self.write_bytes_data(prop_offset + 4, LONG_LEGACY_ERROR.as_bytes())?;
        self.write_bytes_data(prop_offset + 4 + LONG_LEGACY_ERROR.len() as u32, &[0])?;
        self.write_u32_data(prop_offset + LONG_OFFSET_IN_INFO, relative_offset)?;
        let error_len = u32::try_from(LONG_LEGACY_ERROR.len())
            .map_err(|_| PropAreaError::Corrupted("legacy error marker too long"))?;
        self.write_u32_data(
            prop_offset + PROP_SERIAL_OFFSET,
            (error_len << 24) | PROP_INFO_LONG_FLAG,
        )?;

        self.write_bytes_data(long_value_offset, value.as_bytes())?;
        self.write_bytes_data(long_value_offset + value.len() as u32, &[0])?;
        Ok(())
    }

    fn wipe_prop_info(&mut self, prop_offset: u32) -> Result<()> {
        let record = self.read_prop_record(prop_offset)?;
        if record.is_long {
            let value_len = record.value.len() as u32 + 1;
            self.zero_data(record.value_offset, value_len)?;
        }

        let name_len = record.name.len() as u32 + 1;
        self.zero_data(prop_offset + PROP_INFO_SIZE, name_len)?;
        self.zero_data(prop_offset, PROP_INFO_SIZE)?;
        Ok(())
    }

    fn prune_trie(&mut self, offset: u32) -> Result<bool> {
        let node = self.read_node(offset)?;
        let mut is_leaf = true;

        if node.children != 0 {
            if self.prune_trie(node.children)? {
                self.write_u32_data(offset + NODE_CHILDREN_OFFSET, 0)?;
            } else {
                is_leaf = false;
            }
        }
        if node.left != 0 {
            if self.prune_trie(node.left)? {
                self.write_u32_data(offset + NODE_LEFT_OFFSET, 0)?;
            } else {
                is_leaf = false;
            }
        }
        if node.right != 0 {
            if self.prune_trie(node.right)? {
                self.write_u32_data(offset + NODE_RIGHT_OFFSET, 0)?;
            } else {
                is_leaf = false;
            }
        }

        let prop = self.read_u32_data(offset + NODE_PROP_OFFSET)?;
        if is_leaf && prop == 0 {
            if node.namelen != 0 {
                self.zero_data(offset + PROP_TRIE_NODE_HEADER_SIZE, node.namelen + 1)?;
            }
            self.zero_data(offset, PROP_TRIE_NODE_HEADER_SIZE)?;
            return Ok(true);
        }

        Ok(false)
    }

    fn allocate_obj(&mut self, size: u32) -> Result<u32> {
        let aligned = align_up(size, 4);
        let bytes_used = self.bytes_used()?;
        let next = bytes_used
            .checked_add(aligned)
            .ok_or(PropAreaError::AreaFull {
                requested: aligned,
                available: self.data_size.saturating_sub(bytes_used),
            })?;

        if next > self.data_size {
            return Err(PropAreaError::AreaFull {
                requested: aligned,
                available: self.data_size.saturating_sub(bytes_used),
            });
        }

        self.write_u32_abs(BYTES_USED_OFFSET, next)?;
        self.zero_data(bytes_used, aligned)?;
        Ok(bytes_used)
    }

    fn write_u32_abs(&mut self, absolute_offset: u64, value: u32) -> Result<()> {
        self.inner.seek(SeekFrom::Start(absolute_offset))?;
        self.inner.write_all(&value.to_le_bytes())?;
        Ok(())
    }

    fn write_u32_data(&mut self, data_offset: u32, value: u32) -> Result<()> {
        let absolute_offset = self.check_range(data_offset, 4)?;
        self.write_u32_abs(absolute_offset, value)
    }

    fn write_bytes_data(&mut self, data_offset: u32, bytes: &[u8]) -> Result<()> {
        let len = u32::try_from(bytes.len()).map_err(|_| PropAreaError::Corrupted("buffer too large"))?;
        let absolute_offset = self.check_range(data_offset, len)?;
        self.inner.seek(SeekFrom::Start(absolute_offset))?;
        self.inner.write_all(bytes)?;
        Ok(())
    }

    fn zero_data(&mut self, data_offset: u32, len: u32) -> Result<()> {
        let absolute_offset = self.check_range(data_offset, len)?;
        self.inner.seek(SeekFrom::Start(absolute_offset))?;
        let zeros = [0u8; 256];
        let mut remaining = len as usize;
        while remaining > 0 {
            let chunk = remaining.min(zeros.len());
            self.inner.write_all(&zeros[..chunk])?;
            remaining -= chunk;
        }
        Ok(())
    }
}

fn read_u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn cmp_prop_name(one: &str, two: &str) -> std::cmp::Ordering {
    match one.len().cmp(&two.len()) {
        std::cmp::Ordering::Equal => one.as_bytes().cmp(two.as_bytes()),
        ordering => ordering,
    }
}

fn zero_fill<M: Write + Seek>(inner: &mut M, area_size: u64) -> Result<()> {
    inner.seek(SeekFrom::Start(0))?;
    let zeros = [0u8; 4096];
    let mut remaining = area_size;
    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(zeros.len() as u64)).unwrap();
        inner.write_all(&zeros[..chunk])?;
        remaining -= chunk as u64;
    }
    inner.flush()?;
    inner.seek(SeekFrom::Start(0))?;
    Ok(())
}
