# inclean

_[English](README.md) | 简体中文_

一款 C/C++ `#include` 路径规范化工具。

许多老旧的 C/C++ 库以裸文件名 `#include` 头文件
（`#include "bar.h"`），尽管真正的头文件深藏在多层目录下
（`src/internal/bar.h`）。使用者为了消费这样的库，必须把每一个
内部目录都加进 `-I` 列表 —— 既污染了使用者的 include 命名空间，
也破坏了库自身的封装。

`inclean` 在源码层面做一次性规范化：扫描库里的每一个源文件，
把每条 `#include` 改写成能在一组小而显式的允许 include 目录下
干净地解析。`inclean` 跑过之后，使用者只需 `-I` 那些允许目录。

---

## 安装

`inclean` 已发布到 **crates.io**、**PyPI**，并以预编译二进制形式
托管于 **GitHub Releases**。挑你机器上已有的生态系统即可。

### 通过 PyPI（Python wheels —— 无需 Rust 工具链）

三种方式：

```sh
uv tool install inclean      # 隔离环境，速度最快
pipx install inclean         # 隔离环境
pip install inclean          # 装进当前环境
```

Wheel 内含 maturin 构建的原生二进制；`inclean` 会像 `ruff`、`uv`
那样被放进你的 `PATH`。要求 Python ≥ 3.8。

### 通过 cargo

两种方式：

```sh
cargo binstall inclean       # 从 GitHub Releases 下载预编译二进制（不编译）
cargo install inclean        # 从 crates.io 拉源码本地编译
```

`cargo binstall` 从 crates.io 读取 inclean 的 binstall 元数据，再
到本仓库的 GitHub Releases 下载与目标三元组匹配的归档（不编译）。
`cargo install` 则从 crates.io 拉源码到本地编译。

`cargo binstall` 是第三方 cargo 子命令 —— 若尚未安装，请先按
[cargo-bins/cargo-binstall](https://github.com/cargo-bins/cargo-binstall)
的指引装好。

### 预编译二进制（不使用包管理器）

到 [最新 GitHub Release](https://github.com/inaku-Gyan/inclean/releases/latest)
下载你平台对应的归档，把 `inclean`（或 `inclean.exe`）放进 `PATH`。

### 从仓库源码

克隆本仓库并从源码构建：

```sh
git clone https://github.com/inaku-Gyan/inclean.git
cd inclean
cargo install --path .
```

## 快速上手

`inclean` 由放在待清理库根目录的 `inclean.toml` 驱动。典型流程：

```sh
inclean init                # 写一份带注释的 inclean.toml 模板
$EDITOR inclean.toml        # 告诉它你的头文件在哪
inclean check               # 试运行：报告每一处拟改写
inclean diff                # 以 unified diff 形式查看改写
inclean apply               # 就地写入改写
```

`check` / `diff` / `apply` 可额外接受 `[PATHS...]` 用于限制处理范围；
未传时考虑项目根下所有源文件。`-c PATH` 覆盖向上查找 `inclean.toml`
的行为；`-j N` 指定工作线程数。

规则会先检查 `file_paths`、`file_suffixes`、`include_forms` 和
`include_match`。glob 是全字符串锚定的，并使用 literal separator
语义：`foo.h` 只匹配 `foo.h`，`**/foo.h` 才匹配任意深度下的同名文件。
当设置了 `include_directories`，inclean 会继续探测这些字面目录，并应用
`include_on_unresolved`（`error` / `skip` / `allow`）和
`include_on_ambiguous`（`error` / `skip` / `first`）。
如果一条规则只想参与某一侧的改写，可以把整个字段写成
`action = "skip"` 或 `trailing_comment = "skip"`，让这一侧不参与
冲突检查。`keep` 仍会参与冲突检查；`skip` 不参与。没有
`copied_from` 的规则如果省略这两个字段，默认值都是 `skip`。

### 示例

用 `replace` 动作把 `#include "foo.h"` 改写为
`#include "lib/foo.h"`：

```toml
[project]
root = "."
version = "0.3.0-alpha.3"
min_inclean_version = "0.3.0-alpha.3"

[[rule]]
name = "lib-prefix"
file_paths = ["src/**/*"]
include_match = ["foo.h", "bar.h"]
action = { type = "replace", with = "lib/${original}" }
```

完整端到端示例见 [tests/golden_tests/](tests/golden_tests/)。

## 编辑器支持

`inclean.toml` 带 JSON Schema，可用于编辑器补全和校验。`inclean init`
会写入 `#:schema` 行，并写入 `[project].version` 与
`[project].min_inclean_version`。`#:schema` 只服务编辑器；CLI 会独立执行
双向兼容性检查。

```toml
#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v1.2.3/schemas/inclean.toml.schema.json

[project]
root = "."
version = "1.2.3"
min_inclean_version = "1.1.0"
```

（以上版本号仅作示例）

也可以导出一份本地 schema：

```sh
inclean config schema --output inclean.toml.schema.json
```

## 文档

- `inclean init` 生成的 `inclean.toml` 模板是当前最完整的用户侧配置参考。
- **[schemas/inclean.toml.schema.json](../schemas/inclean.toml.schema.json)** ——
  由 Rust 配置结构生成的编辑器 schema。
- **[tests/golden_tests/](../tests/golden_tests/)** —— 可运行的端到端示例，
  覆盖 replace、resolve、copy、suppression、trailing comment、conflict
  和编码保留。
- **[CONTRIBUTING.md](../CONTRIBUTING.md)** —— 工具链、开发流程、
  约定、范围、发布流程。

## 许可证

[BSD 3-Clause](../LICENSE)。
