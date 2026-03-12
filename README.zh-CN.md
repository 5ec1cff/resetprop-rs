# resetprop-rs

[English](README.md)

`resetprop-rs` 是一个用于处理 Android 系统属性存储的 Rust 工具集。

它可以：

- 解析原始 Android `prop_area` 文件
- 读取、写入、更新、删除属性
- 通过 Android `property_contexts` 将属性名解析到正确的 SELinux context
- 检查分配布局、空洞和 dirty-backup 区域
- 对删除后的空洞执行 compact 回收
- 读写 Android persistent property 文件（`persistent_properties` protobuf 格式）
- 同时作为可复用库和命令行工具使用

虽然名字叫 `resetprop-rs`，但它**不是** Magisk `resetprop` 的包装器。它直接理解 Android 属性区和 context 元数据的底层结构。

## 这个项目解决什么问题

Android 原生属性实现本身很强，但在下面这些场景里并不够顺手：

- 离线分析拷贝出来的 `/dev/__properties__`
- 在 Windows / Linux / macOS 主机上调试 Android 数据
- 针对真实或合成 prop area 编写测试
- 不想引入整套 Android 运行时依赖，只想写一个小工具

`resetprop-rs` 保持了对底层布局的兼容，同时把这套能力做成了一个专注、可测试、可复用的 Rust crate 和 CLI。

## 优势

### 1. Rust 重写，做低层二进制处理更稳妥

项目使用 Rust，而不是 C/C++。这并不意味着绝对不会出错，但它确实能减少二进制解析器和原地修改工具里常见的一整类内存安全问题。

对于一个要直接读写底层二进制结构的工具来说，这一点是实打实的优势。

### 2. 支持脱机分析，不依赖设备在线

property context 解析器使用普通文件 I/O，可以直接处理从 Android 系统拷贝出来的数据。

这意味着它非常适合：

- ROM / 镜像分析
- 逆向和问题定位
- CI / 回归测试
- 法证或离线排查
- 手边没有设备、设备没 root、设备不方便连接时的分析工作

### 3. 可观测性比常见属性工具更强

大多数属性工具主要关心 `get` / `set`，但这个项目还能把内部布局直接展示出来。

例如：

- `scan` 可以显示 live object、holes、dirty-backup 是否存在
- 对象会区分类型：`trie-node`、`prop-info`、`long-value`、`dirty-backup`
- `compact` 可以在删除属性后回收空洞

这使得它不仅能“改值”，还能回答“内部到底发生了什么”。对于调试碎片、验证布局假设、观察修改副作用，非常有价值。

### 4. 不只是解析，还能真正修改

这个 crate 不只是只读分析器，还支持实际修改：

- inline value 原地更新
- long value 在条件允许时原地更新
- 删除属性
- 删除后 compact 回收空间

这对于生成测试夹具、做受控实验、离线修改 prop area 都很实用。

### 5. 支持 context 感知路由

主 CLI `sysprop` 能通过 Android property context 元数据，把属性名自动路由到正确的 prop-area 文件。

已支持的 context 存储模式：

- **Serialized**
- **Split**
- **PreSplit**

这意味着它不是只支持某一种 Android 版本布局，而是覆盖了主流的几种 context 组织方式。

### 6. 对低权限和宿主机分析更友好

在枚举 prop-area 文件时，项目会优先利用已知 context 元数据，而不是一味直接遍历目录。对于 Android 上低权限用户无法直接列出 `/dev/__properties__` 的场景，这个设计更实用。

### 7. 既能当库，也能直接当工具用

仓库里同时提供：

- 可复用 Rust library
- `sysprop`：主 CLI，支持 context 路由
- `read_props`：最小原始 prop-area 读取工具
- `write_props`：最小原始 prop-area 写入工具
- `cargo_android_sysprop`：借助 `cargo ndk` + `adb` 构建并推送 `sysprop` 到 Android 的辅助工具

### 8. 有真实用例测试，不只是理论支持

测试覆盖了：

- short / long 属性
- 读 / 写 / 删
- 原地更新规则
- allocation scan
- dirty-backup 检测
- 删除后的 compact
- 基于样例 prop-area 文件的读写测试

这让项目更适合做工程工具，而不只是一次性的格式实验。

## 项目结构

- `src/lib.rs` — 对外导出的库接口
- `src/prop_area.rs` — prop-area 底层解析、修改、scan、compact
- `src/property_context.rs` — Android property context 解析与 context 路由
- `src/persistent_prop.rs` — persistent property protobuf 增删查改（纯 Rust stdlib，无 libc）
- `src/bin/sysprop.rs` — 主 CLI
- `src/bin/read_props.rs` — 简化版原始读取工具
- `src/bin/write_props.rs` — 简化版原始写入工具
- `src/bin/cargo_android_sysprop.rs` — Android 构建 / 推送辅助工具
- `tests/` — 集成测试与夹具

## 构建

```bash
cargo build
cargo test
```

只构建主 CLI：

```bash
cargo build --bin sysprop
```

## 主 CLI：`sysprop`

查看帮助：

```bash
cargo run --bin sysprop -- --help
```

### 基于 context 自动路由的操作

下面这些命令会通过 Android property context 元数据自动找到正确的 prop-area 文件：

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> get ro.build.fingerprint
cargo run --bin sysprop -- --props-dir <PROPS_DIR> set persist.sys.locale en-US
cargo run --bin sysprop -- --props-dir <PROPS_DIR> del persist.sys.locale
cargo run --bin sysprop -- --props-dir <PROPS_DIR> list --show-context
cargo run --bin sysprop -- --props-dir <PROPS_DIR> scan --objects
cargo run --bin sysprop -- --props-dir <PROPS_DIR> compact
```

#### `--persistent` 参数（仅 Android）

`set` 和 `del` 支持 `--persistent` 参数，加上之后会**同时**写入 prop area 和
`/data/property/persistent_properties`，使修改在重启后依然生效：

```bash
# 写入 prop area，同时持久化（重启保留）
sysprop --props-dir <PROPS_DIR> set persist.sys.locale en-US --persistent
# 从 prop area 删除，同时从 persistent storage 中删除
sysprop --props-dir <PROPS_DIR> del persist.sys.locale --persistent
```

`get` 和 `list` 也支持 `--persistent`；加上后会直接读取
`/data/property/persistent_properties`，而不是 prop area。

如果只想处理某一个 context：

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> scan --context u:object_r:build_prop:s0 --objects
cargo run --bin sysprop -- --props-dir <PROPS_DIR> compact --context u:object_r:build_prop:s0
```

在非 Android 宿主机上，如果 context 存储模式是 `Split`，通常还需要加上 `--system-root <ANDROID_ROOT>`。

### Persistent property 文件操作

`persistent-file` 子命令直接操作 Android `persistent_properties` protobuf 文件，全平台可用；
在非 Android 宿主机上需要用 `--path` 指定文件路径。

```bash
# 列出所有 persistent 属性
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties list

# 获取单个属性
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties get persist.sys.locale

# 设置属性（原子写入，重启保留）
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties set persist.sys.locale en-US

# 删除属性
cargo run --bin sysprop -- persistent-file --path /data/property/persistent_properties del persist.sys.locale
```

在 Android 上路径默认为 `/data/property/persistent_properties`，可以省略 `--path`：

```bash
sysprop persistent-file list
sysprop persistent-file get persist.sys.locale
```

### 直接针对单个 area 的操作

下面这些命令直接操作某一个具体 prop-area 文件：

```bash
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop list
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop scan --objects
cargo run --bin sysprop -- area --path tests/fixtures/sample_props.prop compact
```

也可以按 context 名称指定：

```bash
cargo run --bin sysprop -- --props-dir <PROPS_DIR> area --context u:object_r:build_prop:s0 scan --objects
```

## 简化工具

### 读取原始 prop-area 文件

```bash
cargo run --bin read_props -- tests/fixtures/sample_props.prop
cargo run --bin read_props -- tests/fixtures/sample_props.prop ro.product.locale
```

### 写入原始 prop-area 文件

```bash
cargo run --bin write_props -- tests/fixtures/sample_props.prop ro.product.locale=en-US
```

## 部署 `sysprop` 到 Android

如果本机已经安装 `cargo ndk` 和 `adb`：

```bash
cargo run --bin cargo_android_sysprop -- --target aarch64-linux-android --profile release
```

这个辅助工具会完成构建、推送到设备、并设置可执行权限。

## 库用法示例

```rust
use std::fs::File;
use resetprop_rs::PropArea;

let file = File::open("tests/fixtures/sample_props.prop")?;
let mut area = PropArea::new(file)?;

if let Some(info) = area.get_property_info("ro.product.locale")? {
    println!("{} = {}", info.name, info.value);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## 范围与非目标

这个项目专注于理解和操作 Android property-area 数据本身。

它**不是** Android property service 的完整替代品，也不打算完整模拟 init、SELinux 策略执行、以及 Android 整套属性系统的所有运行时行为。

## 当前状态

这个项目已经适合做检查、实验和工具化，尤其适用于“需要看清楚 prop area 里到底有什么，并在可控条件下修改它”的场景。

如果目标是：

- 看懂原始属性区
- 检查布局和碎片
- 离线修改属性
- 写自动化分析或回归工具

那么这个仓库就是围绕这些需求设计的。
