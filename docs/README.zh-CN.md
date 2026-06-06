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

## 为什么使用 inclean？

对于某老旧的库 `some-old-lib`，原本用户需要把其内部的目录也纳入用户自己的编译头文件搜索路径：

```sh
gcc main.c -o main -I third_party -I third_party/some-old-lib/internal
```

![不使用 inclean 的情况](assets/without_inclean.png)

使用 inclean，就能自动清理规范化 `some-old-lib` 中的 `#include` 路径，使得用户不用再包含其内部目录，只需包含顶层目录即可：

```sh
gcc main.c -o main -I third_party
```

![使用 inclean 的效果展示](assets/using_inclean.GIF)

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

### 配置概览

每个 `[[rule]]` 先缩小自己负责的 include 范围，再决定如何处理它们。

- `file_paths` 和 `file_suffixes` 选择源文件。
- `include_forms` 和 `include_match` 选择 include 行。
- `include_directories` 开启头文件解析；`include_resolved_match`
  过滤解析后的头文件路径。
- `action` 负责改写、删除、注释掉、保留或报错；`trailing_comment`
  处理同一行的尾随注释。

多数项目只需要 `include_match` 加一个 `action`。需要统一路径时再配置
`include_directories`；需要处理宏 include 时再看 `macro_rewrite`。glob
规则、宏 include 行为、冲突检查、复制语义和完整字段说明见
[配置语法文档](configuration.zh-CN.md)。

### 示例

用 `replace` 动作把 `#include "foo.h"` 改写为
`#include "lib/foo.h"`：

```toml
[project]
root = "."
version = "0.3.0"
min_inclean_version = "0.3.0"

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
#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v0.4.0-beta.1/schemas/inclean.toml.schema.json

[project]
root = "."
version = "0.4.0-beta.1"
min_inclean_version = "0.4.0-alpha.3"
```

（以上版本号是这个发布线的示例）

也可以导出一份本地 schema：

```sh
inclean config schema --output inclean.toml.schema.json
```

## 文档

- **[配置语法文档](configuration.zh-CN.md)** —— 字段含义、匹配语法、
  actions、复制语义、常量和示例。
- **[schemas/inclean.toml.schema.json](../schemas/inclean.toml.schema.json)** ——
  由 Rust 配置结构生成的编辑器 schema。
- **[tests/golden_tests/](../tests/golden_tests/)** —— 可运行的端到端示例，
  覆盖 replace、resolve、copy、suppression、trailing comment、conflict
  和编码保留。
- **[CONTRIBUTING.md](../CONTRIBUTING.md)** —— 工具链、开发流程、
  约定、范围、发布流程。

## 许可证

[BSD 3-Clause](../LICENSE)。
