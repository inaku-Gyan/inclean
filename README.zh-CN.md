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
已发布的目标三元组：

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

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

除 `explain` 外的每个命令都接受一个可选的 `[DIR]` 参数 —— 指向
存放根 `inclean.toml` 的目录，默认为 `.`。

### 示例

考虑一个「扁平」库：头文件真正位于
`include/mylib/internal/foo.h`，但内部 `#include` 仅用 basename。
[tests/fixtures/flat-library/](tests/fixtures/flat-library/) 这个
fixture 自带如下配置：

```toml
[project]
root = "."

[[rule]]
name = "base"
paths = ["src/**", "include/**"]
forms = ["quote"]
allowed_include_dirs = ["include"]
original_include_dirs = ["include/mylib/internal"]
```

源码中的 `#include "foo.h"` 会被改写为
`#include "mylib/internal/foo.h"` —— 使用者只需 `-Iinclude`。

## 命令

| 命令                                     | 用途                                                 |
| ---------------------------------------- | ---------------------------------------------------- |
| `inclean init [DIR]`                     | 生成带注释的 `inclean.toml` 模板。拒绝覆盖已有文件。 |
| `inclean check [DIR] [-l/--level LEVEL]` | 三个深度之一的只读检查。永不写入。                   |
| `inclean diff [DIR]`                     | 以 unified diff 形式打印每一处拟改写。               |
| `inclean apply [DIR]`                    | 就地应用改写。若存在任何规则树冲突，整体拒绝执行。   |
| `inclean explain FILE [INCLUDE]`         | 逐层追踪指定 `#include` 被哪条规则匹配 —— 调试辅助。 |

`inclean check` 可在三个层级之一运行（`-l config | rules | full`，
默认 `full`）。每一层是上一层的严格超集；完整说明见
[docs/configuration.md](docs/configuration.md#inclean-check-levels)。

## 文档

- **[docs/configuration.md](docs/configuration.md)** —— 完整的
  `inclean.toml` schema：五层匹配模型、继承、`@std.*` 常量、动作、
  占位符、退出码。
- **[docs/architecture.md](docs/architecture.md)** —— 代码层面的
  架构：模块图、流水线阶段、关键不变量。
- **[CONTRIBUTING.md](CONTRIBUTING.md)** —— 工具链、开发流程、
  约定、范围、发布流程。
- **[CHANGELOG.md](CHANGELOG.md)** —— 发布历史。

## 状态

`0.1.1` —— 当前版本。包含对 `trailing_comment` 动作 schema 的
不兼容改动；迁移说明详见 [CHANGELOG.md](CHANGELOG.md)。v1 功能
已完整。

## 许可证

[BSD 3-Clause](LICENSE)。
