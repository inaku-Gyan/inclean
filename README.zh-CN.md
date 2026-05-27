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

`check` / `diff` / `apply` 可额外接受 `[PATHS...]` 用于限制处理范围；
未传时考虑项目根下所有源文件。`-c PATH` 覆盖向上查找 `inclean.toml`
的行为；`-j N` 指定工作线程数。

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

## 命令

| 命令                                                                  | 用途                                                                                                                                              |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `inclean init [PATH]`                                                 | 生成带注释的 `inclean.toml` 模板。`inclean config new` 的别名。                                                                                   |
| `inclean check [config\|unfixable\|all] [-c PATH] [-j N] [PATHS...]`  | 只读检查。`config` 仅校验配置文件；`unfixable` 只报告无法自动修复的违规；`all`（默认）报告每一处 per-include 结果。                               |
| `inclean diff [-o PATH] [-c PATH] [-j N] [PATHS...]`                  | 以 unified diff 形式打印每一处拟改写。`-o` 写入文件而非 stdout。                                                                                  |
| `inclean apply [-c PATH] [-j N] [PATHS...]`                           | 就地应用改写。无 unfixable 违规的文件被写入；存在违规（error / conflict / evaluation_failure）的文件整体跳过，最后打印一份违规详情报告。          |
| `inclean config check [-c PATH]`                                      | `inclean check config` 的别名。                                                                                                                   |
| `inclean config new [PATH]`                                           | `inclean init` 的别名。                                                                                                                           |
| `inclean config schema [-o PATH] [--check]`                           | 输出 / 校验 `inclean.toml` 的 JSON Schema。`--check` 模式要求 `-o`，若 schema 偏移则以非零状态码退出。                                            |

未指定 action 的规则默认采用 `{ type = "keep", output_form = "preserve" }`
（即不动作）。

## 文档

- **[docs/configuration.md](docs/configuration.md)** —— 完整的
  `inclean.toml` schema：四层匹配模型、`copied_from` 复制语义、
  `@std.*` 常量、6 种动作、占位符、退出码。
- **[docs/architecture.md](docs/architecture.md)** —— 代码层面的
  架构：模块图、流水线阶段、关键不变量。
- **[CONTRIBUTING.md](CONTRIBUTING.md)** —— 工具链、开发流程、
  约定、范围、发布流程。
- **[CHANGELOG.md](CHANGELOG.md)** —— 发布历史。

## 状态

`0.3.0` —— 当前版本。围绕 `copied_from`（单层、可传递的复制语义，
替代原来的 `extends` AND 合并）、4 层匹配（`file_paths` /
`file_suffixes` / `match_forms` / `include_match`）、六种动作
（`resolve` / `replace` / `keep` / `remove` / `comment_out` /
`error`）、`suppression_comments_regex` 屏蔽区域、新的
`trailing_comment.transform` 模型，以及"按最终行文本判定冲突"
（而非依赖规则树不变量）等设计做了大规模重构。CLI 新增
`check unfixable` / `check all`、`[PATHS...]` 过滤、以及通过
`inclean diff -o` 将统一 diff 写入文件。inclean 处于 pre-1.0
beta，breaking schema 变更不提供迁移 shim；详见
[CLAUDE.md](CLAUDE.md#pre-10-backward-compat-policy)。

## 许可证

[BSD 3-Clause](LICENSE)。
