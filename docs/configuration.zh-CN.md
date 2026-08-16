# inclean 配置语法

_[English](configuration.md) | 简体中文_

`inclean` 读取名为 `inclean.toml` 的 TOML 配置文件。默认情况下，CLI
会从当前目录向上查找该文件；也可以用 `-c PATH` 指定具体文件。未知字段会被拒绝。

```toml
#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v0.4.1/schemas/inclean.toml.schema.json

[project]
root = "."
version = "0.4.1"
min_inclean_version = "0.4.0-alpha.3"

[[rule]]
name = "rewrite-public-headers"
file_paths = ["src/**/*"]
include_forms = ["quote"]
include_match = ["**/*.h"]
include_directories = ["include"]
action = { type = "resolve", relative_to = "include", output_form = "quote" }
```

## 文件结构

一份 `inclean.toml` 包含一个 `[project]` 表，以及若干个重复的 `[[rule]]`
表。规则会按声明顺序解析和执行。同一条 `#include` 可以匹配多条规则；
`inclean` 会比较每条匹配规则最终会写出的文本，如果结果不一致就报告冲突。

`action` 负责改写 include 路径或引号形式。`trailing_comment` 负责改写同一行
尾随注释。这两个维度会分开做冲突检查。整个字段写成 `"skip"` 表示该维度不参与
冲突检查；`keep` 仍然参与，因为它代表一个显式结果。

## `[project]`

| 字段                  | 必填 | 默认值 | 含义                                                                                       |
| --------------------- | ---- | ------ | ------------------------------------------------------------------------------------------ |
| `root`                | 否   | `"."`  | 项目根目录，相对配置文件所在目录解析。所有规则路径都相对这个根目录。必须指向已存在的目录。 |
| `version`             | 是   | 无     | 写出这份配置的 CLI 对应的配置 schema 版本。                                                |
| `min_inclean_version` | 是   | 无     | 预期能正确解析这份配置的最低 CLI 版本。                                                    |

`#:schema` 是可选的，只服务编辑器补全和校验。CLI 自己会使用上面的 version 字段做
兼容性检查。

## 规则匹配流程

一条规则只有在所有已配置层都通过时才算匹配：

1. `file_paths` 和 `file_suffixes` 选择源文件。
2. `suppression_comments_regex` 可以让这条规则跳过某些源码行。
3. `include_forms` 选择 quote、angle 或 macro include。
4. `include_match` 匹配去掉分隔符后的 include 参数。
5. 如果 `include_directories` 非空，则解析到真实头文件，并用
   `include_resolved_match` 过滤解析后的路径。

几类共享语法集中说明在这里：[glob 语法](#glob-syntax)、
[placeholders](#placeholders) 和 [内置常量](#constants)。

<a id="glob-syntax"></a>

### Glob 语法

`file_paths`、`include_match` 和 `include_resolved_match` 使用有序 signed
glob 列表。

- 模式是全字符串锚定的：`foo.h` 只匹配 `foo.h`；任意深度请写 `**/foo.h`。
- `*` 不跨 `/`；`**` 可以跨 `/`。
- 未转义的开头 `!` 表示取反。
- 靠后的匹配会覆盖靠前的匹配。
- 字面开头感叹号在 TOML 单引号中写 `'\!weird.h'`，在双引号中写
  `"\\!weird.h"`。
- `include_directories` 不是 glob 列表，它的元素是项目根下的字面目录。

### 规则身份和复制

| 字段          | 必填 | 默认值 | 含义                                               |
| ------------- | ---- | ------ | -------------------------------------------------- |
| `name`        | 是   | 无     | 全局唯一规则名，用于诊断输出。                     |
| `copied_from` | 否   | 无     | 要继承的前置规则名。不能前向引用，也不能复制自己。 |

设置 `copied_from` 后，子规则省略的顶层字段会继承父规则已经解析后的值。子规则写出的
顶层字段会整体替换父规则字段。对于对象字段，如果子规则写了外层对象，省略的内层字段会
回到默认值，而不是自动继承父规则。需要继承时显式使用 `${copied}`：

```toml
[[rule]]
name = "base"
file_paths = ["src/**/*"]
include_match = ["**"]
action = { type = "resolve", relative_to = "include" }

[[rule]]
name = "only-public"
copied_from = "base"
include_match = ["${copied}", "!private/**"]
action = { type = "resolve", relative_to = "${copied}", output_form = "quote" }
```

合法的 `${copied}` 形式：

| 位置           | 语法                        | 含义                                                                                        |
| -------------- | --------------------------- | ------------------------------------------------------------------------------------------- |
| 整个对象字段   | `action = "${copied}"`      | 复制父规则的整个对象。适用于 `action`、`trailing_comment` 和 `suppression_comments_regex`。 |
| 数组元素       | `["${copied}", "!x/**"]`    | 在当前位置展开父规则的整个数组。                                                            |
| 内层字符串字段 | `relative_to = "${copied}"` | 复制父规则对应的标量字段。复制动作特有字段时，父子 action 必须是同一变体。                  |

`${copied}` 在复制解析阶段求值，和运行时 [placeholders](#placeholders) 是两套机制。

### 源文件选择

| 字段            | 默认值                                         | 含义                                                              |
| --------------- | ---------------------------------------------- | ----------------------------------------------------------------- |
| `file_paths`    | `["**/*"]`                                     | 有序 signed [glob 列表](#glob-syntax)，匹配项目根相对源文件路径。 |
| `file_suffixes` | `["@std.c.extensions", "@std.cpp.extensions"]` | 字面扩展名，包含开头的点。可使用 [内置常量](#constants)。         |

精确字面 `file_paths` 匹配会跳过 `file_suffixes`；通配符匹配必须继续通过
`file_suffixes`。

### 抑制区域

`suppression_comments_regex` 用于标记这条规则不能编辑的源码行。它可以省略，可以是对象，
也可以整个字段写成 `"${copied}"`。

```toml
suppression_comments_regex = {
    block_start = "^inclean: off$",
    block_end = "^inclean: on$",
    line = "^inclean: skip$",
}
```

| 内层字段      | 含义                                                 |
| ------------- | ---------------------------------------------------- |
| `line`        | 只抑制 probe 文本匹配该正则的行。                    |
| `block_start` | 从匹配行开始进入不可编辑块。                         |
| `block_end`   | 在匹配行结束不可编辑块。省略时，块会持续到文件末尾。 |

匹配时，`inclean` 会在存在 `//` 或同一行 `/* ... */` 分隔符时去掉分隔符，再 trim
空白，然后把该文本交给正则。非注释行会用 trim 后的原始文本匹配。正则字符串可使用
[内置常量](#constants)。

### Include 匹配

| 字段            | 默认值          | 含义                                                                                                                                   |
| --------------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `include_forms` | `["quote"]`     | 规则匹配的 include 形式：`"quote"` 对应 `#include "x.h"` 或 `#define X "x.h"`，`"angle"` 对应 `<x.h>`，`"macro"` 对应未展开的 `#include X`。 |
| `macro_rewrite` | `"definitions"` | 展开宏 include 的改写目标：`"definitions"` 改写匹配到的 `#define` 头文件 token；`"use_site"` 改写 `#include MACRO` 参数且要求所有分支结果一致。 |
| `include_match` | `["**"]`        | 有序 signed [glob 列表](#glob-syntax)，匹配去掉引号或尖括号后的 include 参数，例如 `mylib/foo.h`。                                     |

`#include MACRO` 有特殊处理。`inclean` 会扫描符合规则文件选择条件的源文件，寻找替换列表
正好是一个头文件 token 的简单对象宏：

```c
#define MY_HEADER "foo.h"
#define SYS_HEADER <foo.h>
```

同名的每个 header-like 定义都会被当作一个可能分支。匹配和 `${current_file}` 使用
`#include MACRO` 的使用点上下文；定义只提供有效的 quote/angle 形式、路径和可编辑范围。
默认 `macro_rewrite = "definitions"` 会分别改写每个匹配分支的 `#define` 值。
`macro_rewrite = "use_site"` 会改写 `#include MACRO` 参数本身，并要求所有匹配分支得到
同一个最终 include 参数。未展开的宏 include 仍可通过 `include_forms = ["macro"]`
匹配，但非 `skip` 的 action 会报告错误。

### 头文件解析

| 字段                     | 默认值    | 含义                                                                                                                                  |
| ------------------------ | --------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `include_directories`    | `[]`      | 项目根下的字面目录路径。不是 [glob](#glob-syntax)，没有隐式 `/**`，也没有 `.gitignore` 语义。                                         |
| `include_resolved_match` | `["**"]`  | 有序 signed [glob 列表](#glob-syntax)，匹配解析后的头文件路径；能相对项目根时使用项目根相对路径。`include_directories` 为空时无效果。 |
| `include_on_unresolved`  | `"error"` | 找不到候选头文件时的策略：`"error"`、`"skip"` 或 `"allow"`。                                                                          |
| `include_on_ambiguous`   | `"error"` | 多个 include 目录都能解析同一个 include 参数时的策略：`"error"`、`"skip"` 或 `"first"`。                                              |

解析时会按如下形式探测每个目录：

```text
<project root>/<include_directories item>/<include argument>
```

`include_on_unresolved = "allow"` 会让规则在没有解析到头文件时仍保持匹配，适合非
`resolve` 动作。它不能和 `action = { type = "resolve", ... }` 一起使用。
`include_on_ambiguous = "first"` 会按 `include_directories` 的声明顺序选第一个匹配项。

<a id="placeholders"></a>

## Placeholders

运行时 placeholders 会在规则匹配之后，在 action 和尾随注释模板字符串中展开：

| Placeholder       | 在 action 模板中                      | 在尾随注释模板中                       |
| ----------------- | ------------------------------------- | -------------------------------------- |
| `${current_file}` | 正在编辑文件的项目根相对路径。        | 正在编辑文件的项目根相对路径。         |
| `${original}`     | 去掉引号或尖括号后的原 include 参数。 | 去掉分隔符并 trim 后的原尾随注释正文。 |

支持的 action 模板字段包括 `action.relative_to`、`action.with` 和
`action.message`。支持的尾随注释模板字段包括
`trailing_comment.transform.action.with` 和 `.message`。

## Actions

`action` 可以是 `"skip"`、`"${copied}"` 或带 `type` 的对象。如果一条规则及其所有父规则
都没有设置 `action`，有效值就是 `"skip"`。

共享字段和值：

- `output_form`：`"quote"`、`"angle"` 或 `"preserve"`；支持它的动作默认
  `"preserve"`。
- `message`：各 action 变体都接受的可选字符串。`error` 会把它作为用户可见错误文本；
  其他变体目前不会用它改变改写结果。
- action 字符串可使用 [placeholders](#placeholders) 和 [内置常量](#constants)。

| Action      | 语法                                                                         | 效果                                                                                                                                  |
| ----------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| skip        | `action = "skip"`                                                            | 规则仍可匹配并执行 `trailing_comment`，但不贡献 action 候选结果。                                                                     |
| copy        | `action = "${copied}"`                                                       | 复制父规则的整个 action。要求设置 `copied_from`。                                                                                     |
| resolve     | `{ type = "resolve", relative_to = "...", output_form = "quote" }`           | 改写为 `include_directories` 选中的头文件路径，并让结果相对 `relative_to`。`relative_to = "${current_file}"` 表示当前源文件所在目录。 |
| replace     | `{ type = "replace", with = "lib/${original}", output_form = "quote" }`      | 用 `with` 替换 include 参数。不需要头文件解析。                                                                                       |
| keep        | `{ type = "keep", output_form = "angle" }`                                   | 保留 include 参数；可选择改变 quote/angle 形式。                                                                                      |
| remove      | `{ type = "remove", keep_blank_line = false, keep_trailing_comment = true }` | 删除整条 include 行。默认不保留空行，并把识别到的尾随注释保留到单独一行。                                                             |
| comment_out | `{ type = "comment_out", style = "//" }`                                     | 注释掉整条 include 行。`style` 为 `"//"` 或 `"/**/"`，默认 `"//"`。                                                                   |
| error       | `{ type = "error", message = "..." }`                                        | 报告一个配置指定的错误。                                                                                                              |

`resolve`、`replace` 和 `keep` 可以配合 `trailing_comment` 使用。`remove`、
`comment_out` 和 `error` 会忽略 `trailing_comment`。

## 尾随注释

`trailing_comment` 可以是 `"skip"`、`"${copied}"` 或对象。如果一条规则及其所有父规则
都没有设置它，有效值就是 `"skip"`。

```toml
trailing_comment = {
    transform = {
        match_styles = ["//", "/**/"],
        content_regex = "^IWYU pragma:",
        action = { type = "keep", output_style = "//" },
    },
    append_if_absent = "  // IWYU pragma: export",
}
```

只有同一行的 `// ...` 和闭合在同一行的 `/* ... */` 会被视为尾随注释。跨行块注释会原样
保留，也不会触发 `append_if_absent`。

| 字段               | 默认值 | 含义                                                                                       |
| ------------------ | ------ | ------------------------------------------------------------------------------------------ |
| `transform`        | 无     | 当已有尾随注释匹配时执行的可选转换。                                                       |
| `append_if_absent` | 无     | 当 action 结果没有尾随注释时追加的字面文本。需要自己包含前导空白和注释分隔符。必须是单行。 |

`transform` 字段：

| 字段            | 默认值           | 含义                         |
| --------------- | ---------------- | ---------------------------- |
| `match_styles`  | `["//", "/**/"]` | 允许匹配的已有注释风格。     |
| `content_regex` | `".*"`           | 匹配 trim 后注释正文的正则。 |
| `action`        | 必填             | 下列尾随注释 action 之一。   |

尾随注释 action：

| Action  | 语法                                                               | 效果                                     |
| ------- | ------------------------------------------------------------------ | ---------------------------------------- |
| replace | `{ type = "replace", with = "IWYU: export", output_style = "//" }` | 替换注释正文。                           |
| keep    | `{ type = "keep", output_style = "preserve" }`                     | 保留注释正文；可选择改变注释分隔符风格。 |
| remove  | `{ type = "remove" }`                                              | 删除该尾随注释。                         |
| error   | `{ type = "error", message = "..." }`                              | 报告不可修复的尾随注释错误。             |

`output_style` 可为 `"//"`、`"/**/"` 或 `"preserve"`，默认 `"preserve"`。
尾随注释模板字符串可使用 [placeholders](#placeholders) 和 [内置常量](#constants)。

<a id="constants"></a>

## 内置常量

内置常量以 `@std.` 开头。

在 `file_suffixes` 中，某个元素如果正好是 `"@name"`，会把该常量列表展开进当前位置：

```toml
file_suffixes = ["@std.c.extensions", "@std.cpp.extensions"]
```

在 suppression 正则、action 模板、尾随注释模板等标量字符串字段中，`@name` 会被替换到
字符串里。列表常量会变成已经转义的正则 alternation。写字面 `@` 请用 `@@`。

`include_match` 和 `include_resolved_match` 是 glob 列表，目前不会展开 `@std.*` 常量。

可用的列表常量：

| 常量                                                                                                                                                                         | 含义                                                  |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `@std.c.header_extensions`                                                                                                                                                   | `.h`                                                  |
| `@std.c.source_extensions`                                                                                                                                                   | `.c`                                                  |
| `@std.c.extensions`                                                                                                                                                          | C 头文件和源文件扩展名。                              |
| `@std.cpp.header_extensions`                                                                                                                                                 | `.hh`、`.hpp`、`.hxx`、`.h++`                         |
| `@std.cpp.source_extensions`                                                                                                                                                 | `.cc`、`.cpp`、`.cxx`、`.c++`、`.inl`、`.ipp`         |
| `@std.cpp.extensions`                                                                                                                                                        | C++ 头文件和源文件扩展名。                            |
| `@std.c89.system_headers`、`@std.c95.system_headers`、`@std.c99.system_headers`、`@std.c11.system_headers`、`@std.c17.system_headers`、`@std.c23.system_headers`             | C 标准库头文件集合，按标准版本累积。                  |
| `@std.cpp98.system_headers`、`@std.cpp11.system_headers`、`@std.cpp14.system_headers`、`@std.cpp17.system_headers`、`@std.cpp20.system_headers`、`@std.cpp23.system_headers` | C++ 标准库头文件集合，按标准版本累积。                |
| `@std.cpp.c_compat_headers`                                                                                                                                                  | C 兼容形式的 C++ 头文件名，例如 `cstdio`、`cstdlib`。 |

在正则字符串里，任何列表常量也可以写成 `@name_or` 来显式请求 alternation，例如
`@std.cpp17.system_headers_or`。

## 常见写法

把 include 改写成使用者只需要 `-I include` 的形式：

```toml
[[rule]]
name = "public-headers"
file_paths = ["src/**/*", "include/**/*"]
include_forms = ["quote"]
include_match = ["**/*.h"]
include_directories = ["include"]
action = { type = "resolve", relative_to = "include", output_form = "quote" }
```

不解析头文件，只给选中的 include 加前缀：

```toml
[[rule]]
name = "lib-prefix"
file_paths = ["src/**/*"]
include_match = ["foo.h", "bar.h"]
action = { type = "replace", with = "lib/${original}" }
```

保留标准库 include：

```toml
[[rule]]
name = "stdlib"
include_forms = ["angle"]
include_match = ["vector", "string", "memory", "utility"]
action = { type = "keep" }
```

不改 include 路径，只新增或规范化尾随注释：

```toml
[[rule]]
name = "export-private"
include_directories = ["include"]
include_resolved_match = ["include/private/**"]
include_on_unresolved = "skip"
action = "skip"
trailing_comment = {
    transform = { action = { type = "replace", with = "IWYU pragma: export" } },
    append_if_absent = "  // IWYU pragma: export",
}
```
