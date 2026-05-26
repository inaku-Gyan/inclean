# Refactor Instructions

## Config File

1. 保留原来的 `@std.*` 命名空间。这次重构中所有原 `@std.c_extensions` 之类的常量改为 `@std.c.extensions` 等带子命名空间的写法（见下文"内置占位符"小节）。
2. 把原来的正则表达式改为 glob pattern，使用 `globset` crate 来匹配
3. 把原来的继承逻辑改为复制逻辑，使用 `copied_from` 字段来指定要复制的规则名称。被复制的规则必须在配置文件中定义，并且必须在当前规则之前定义（按声明顺序）。如果被复制规则本身也使用了 `copied_from`，当前规则拿到的是它**完成复制并应用覆盖之后的最终结果**（transitive copy）。复制时会把被复制规则的所有字段（包括 `name` 字段）复制到当前规则中，然后再覆盖当前规则中指定的字段。这样可以避免继承带来的复杂性和不确定性，同时也更符合用户的预期。
4. 可选字段即有默认值的字段，与必填字段相对。nullable 字段一定是默认值为 `null` 的可选字段，但可选字段不一定是 nullable 字段。

用 TS 类型定义如下（用 `?` 表示可选字段）：

```ts
type Rule = {
  name: string;
  copied_from?: string | null; // 只支持字面量（已经存在的规则名称）。Defaults to `null` (not copied).
  file_paths?: string[]; // glob pattern. Defaults to `["**/*"]` (匹配所有文件).
  file_suffixes?: string[]; // 只支持字面量和内置占位符. Defaults to `["@std.c.extensions", "@std.cpp.extensions"]`.
  suppression_comments_regex?: {
    // 匹配时按行匹配，不区分 // 和 /* */ 注释。匹配时，会把该行首尾的空白去掉再进行正则表达式匹配
    block_start?: string | null; // regex pattern. Defaults to `null` (disabled).
    block_end?: string | null; // regex pattern. Defaults to `null` (disabled).
    line?: string | null; // regex pattern. Defaults to `null` (disabled).
  };
  match_forms?: ("quote" | "angle" | "macro")[]; // Defaults to `["quote"]`. v1 中匹配到 `"macro"` 形式的 include 一律报错（即使 action 不是 `error`）；该枚举值保留以便未来扩展。
  include_match?: string[]; // glob pattern. Defaults to `["**"]` (匹配所有 include 语句).
  include_directories?: string[]; // glob pattern
  action:
    | {
        type: "resolve";
        relative_to: string; // 只支持字面量和内置占位符. 可用 `${current_file}` 来表示当前 include 语句所在的文件路径
        output_form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string; // 可选。 Defaults to `""` (empty string).
      }
    | {
        type: "replace";
        with: string; // 只支持字面量和内置占位符
        output_form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string;
      }
    | {
        type: "keep";
        output_form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string;
      }
    | {
        type: "remove";
        keep_blank_line?: boolean; // Defaults to `false`. If `true`, the blank line will be kept after removing the include statement.
        keep_trailing_comment?: boolean; // Defaults to `true`. If `true`, the trailing comment will be kept after removing the include statement.
        message?: string;
      }
    | {
        type: "comment_out"; // 把整行注释掉，保留原 include 文本和 trailing comment 不动（被注释符号包住）。
        style?: "//" | "/**/"; // Defaults to `"//"`. 选择用行注释还是块注释包住整行。
        message?: string;
      }
    | {
        type: "error";
        message?: string;
      };
  trailing_comment?: {
    transform?: {
      match_styles?: ("//" | "/**/")[]; // Defaults to `["//", "/**/"]`.
      content_regex: string; // 把该行首尾的空白去掉再进行正则表达式匹配. Defaults to `".*"` (匹配所有 trailing comment).
      action:
        | {
            type: "replace";
            with: string; // 只支持字面量和内置占位符
            output_style?: "//" | "/**/" | "preserve"; // Defaults to `"preserve"`.
            message?: string;
          }
        | {
            type: "keep";
            output_style?: "//" | "/**/" | "preserve"; // Defaults to `"preserve"`.
            message?: string;
          }
        | {
            type: "remove";
            message?: string;
          }
        | {
            type: "error";
            message?: string;
          };
    };
    append_if_absent?: string | null; // 只支持字面量和内置占位符. Defaults to `null` (disabled). 用户需要写完整的注释内容，比如 `" // comment"` 或 `" /** comment */"`，工具不会自动添加注释符号和前导空格。
  };
};
```

内置占位符：

- 特殊：
  - `${copied}`: 表示"取被复制规则的该字段的最终值"。任何字段类型都合法：
    - 出现在标量字段中（如 `relative_to = "${copied}"`）：整体替换为父值
    - 出现在对象字段中（如 `transform = "${copied}"`）：整体替换为父值
    - 出现在数组元素位置（如 `file_suffixes = ["${copied}", ".h"]`）：把父数组在该位置 splat 展开
    - 合法性约束：
      - 仅在当前规则有 `copied_from` 时可用；否则 check 时报错（语义错误）
      - splat 上下文下，若父值是空数组或字段从未显式设置：展开为空（不产生元素），不报错
      - 标量/对象上下文下，若父值是 `null` 而当前字段类型不可为 null：check 时报错
- 常量（仅可作为字面字符串数组的元素出现）：
  - `@std.c.extensions`
  - `@std.cpp.extensions`
  - `@std.c.header_extensions`
  - `@std.cpp.header_extensions`
  - `@std.c.source_extensions`
  - `@std.cpp.source_extensions`
- 路径相关：
  - `${current_file}`: 代表当前 include 语句所在的文件路径
  - `${original}`: 代表原来的 trailing comment 内容（不包括注释符号和前导空格）或原来的 include 语句内容（不包括引号或尖括号）

## 复制（copy）语义：覆盖与重置

子规则字段相对父规则的解析规则：

- **顶层字段**省略 → 从父规则继承该字段的最终值
- **顶层字段**显式给出 → 等价于"从零开始重新指定"该字段；**不会**与父规则做字段级合并
- **嵌套对象内的字段**省略 → 使用该字段的默认值（通常即 `null` / disabled），**不会**从父规则继承
- 任何位置出现 `${copied}` → 显式取父规则该位置的最终值

这种"顶层默认继承、内层默认重置"的非对称设计的好处是：所有简单场景（整对象沿用、整对象重写）成本为零；"基于父对象只改一两个内字段、其余沿用"时需要逐一 `${copied}`——稍啰嗦但完全无歧义、不会有隐式继承漏审。

例：父规则 `foo` 含 `suppression_comments_regex = {block_start, block_end, line}` 三个 inner 字段。

```toml
# 子规则 1：整体沿用父值——直接省略，无需任何写法
[[rule]]
name = "child_1"
copied_from = "foo"
# 不写 suppression_comments_regex 即可

# 子规则 2：保留 line，禁用前两者
[[rule]]
name = "child_2"
copied_from = "foo"
suppression_comments_regex = {
    line = "${copied}",
    # block_start, block_end 省略 = 默认 null = disabled
}

# 子规则 3：替换 block_start，沿用 line，禁用 block_end
[[rule]]
name = "child_3"
copied_from = "foo"
suppression_comments_regex = {
    block_start = "^NEW_PATTERN$",
    line = "${copied}",
    # block_end 省略 = 默认 null = disabled
}
```

示例：

```toml
[project]
root = "."
version = "0.3.0" # 创建该配置文件的 inclean 版本。仅在 `inclean config new` 时写入，之后永不自动更新；用户也不应手动修改
min_inclean_version = "0.3.0" # 能解析该配置文件的最低 inclean 版本。规则同上：创建时写入，永不自动更新
# 一次 inclean 更新有三种可能：
# 1. 完全不改 config schema 和语义
# 2. 向后兼容的改动
# 3. 向后不兼容的改动
# inclean CLI 内置记录三个版本号：
# 1. `cli.compat_min`：当前 CLI 能兼容的最低 config 版本（也就是那个版本有不兼容其上一个版本的改动）
# 2. `config.compat_min`：从该 config 看，能兼容它的最低 CLI 版本（即 `min_inclean_version` 字段）
# 3. `cli.current` / `config.version`：当前 CLI 版本 / 创建该配置时的 CLI 版本
#
# config check 通过的判定（其他情况一律报错退出）：
#     cli.compat_min <= config.version  AND  config.compat_min <= cli.current

[[rule]]
name = "foo"
file_paths = ["Drivers/**"] # glob pattern
file_suffixes = ["@std.c.extensions"] # 只支持字面量和内置占位符
suppression_comments_regex = {
    block_start = "^USER CODE BEGIN.*$",
    block_end = "^USER CODE END.*$",
    line = "^inclean: skip$",
}
include_directories = ["Drivers/STM32F4xx_HAL_Driver/Inc", "Drivers/CMSIS/Include", "Drivers/CMSIS/Device/ST/STM32F4xx/Include"]
action = {
    type = "resolve",
    relative_to = "${current_file}",
}
trailing_comment = {
    append_if_absent = " // IWYU pragma: export",
}

[[rule]]
name = "bar"
copied_from = "foo" # 复制 foo 规则的所有字段
# 以下字段会覆盖 foo 规则中对应的字段
file_paths = ["src/**"]
suppression_comments_regex = {
    line = "${copied}", # 沿用 foo 的 line regex，即 `^inclean: skip$`
    # 顶层 suppression_comments_regex 字段被显式给出 → 内层字段不再继承父值。
    # block_start / block_end 省略 → 默认 null → 在 bar 上 disabled。
}
action = {
    type = "resolve",
    relative_to = "Drivers",
    output_form = "angle", # 不沿用 foo 规则的 output_form，而是覆盖为 "angle"
}

[[rule]]
name = "baz"
file_paths = ["cpp/include/**"] # 只限制在 cpp/include 和 cpp/src 目录下生效
file_suffixes = [".hpp", ".cpp"] # 只限制在 .hpp 和 .cpp 文件中生效
include_directories = ["cpp/include", "cpp/include/private"]
action = {
    type = "resolve",
    relative_to = "cpp/include",
    output_form = "quote",
}

[[rule]]
name = "qux"
copied_from = "baz"
file_paths = ["cpp/src/**"]
include_directories = ["cpp/include/private"]
action = {
    type = "error",
    message = "Do not include headers from cpp/include/private",
}
```

## CLI Commands

checks:

1. check config syntax and logic: required fields, inheritance, placeholders, etc.
2. rule conflicts: a single include sentence should not match multiple rules (engine needed)
3. rule violations: is there any include sentence that violates the rules (engine needed)
4. error: hit a rule with `action.type = "error"` or `trailing_comment.transform.action.type = "error"`

```sh
inclean check config [-c|--config <PATH>]
inclean check [unfixable|all] [-c|--config <PATH>] [-j|--jobs <N>] [PATHS...]
```

- `config` 只检查配置文件的语法和逻辑错误（比如必填字段缺失、继承循环、占位符错误、版本不兼容等）。`inclean config check` 是它的 alias。忽略传入的 `-j` 参数和 `PATHS` 参数。
  - 额外规则：数组类字段（如 `match_forms`、`file_paths`、`file_suffixes`、`include_match` 等）出现**用户字面写出**的重复元素时报 warning（不阻塞 check 通过）。`${copied}` splat 展开后产生的重复元素不算（即不警告）——这种情况通常是用户在父规则的基础上追加，与父规则已有的某个元素重合是合理意图。
- `unfixable` = `config` + 无法自动修复的违规。判定标准：只要 hit 到任何一条配置了 `action.type = "error"` 或 `trailing_comment.transform.action.type = "error"` 的规则，就算 unfixable；rule conflict 也算 unfixable。
- `all` = `unfixable` + 可自动修复的违规。默认值。

打印所有违规的详细信息（包括文件路径、行号、违规的 include 语句、违规类型、违规的规则名称等），并以非零状态码退出。如果没有违规，则以零状态码成功退出。

```sh
inclean apply [-c|--config <PATH>] [-j|--jobs <N>] [PATHS...]
```

- `-c` 传入文件路径. 如果不传入，则默认从当前目录开始向上自动发现 `inclean.toml` 配置文件。
- `PATHS` 传入一个或多个路径。只限制这些路径下的文件被检查和修改。路径可以是文件或目录。
- 如果 `PATHS` 为空，则不限制任何路径，对文件系统中任何路径生效

apply 正式启动引擎前，自动先检查 config（类似 `inclean config check`）。如果 config 有错误，则直接报错退出，不进入引擎阶段。
自动修复所有可修复的违规。
修复结束后，如果有无法修复的违规，则最后报错显示这些违规的详细信息（包括文件路径、行号、违规的 include 语句、违规类型、违规的规则名称等），并以非零状态码退出。如果没有无法修复的违规，则以零状态码成功退出。

```sh
inclean diff [-o|--output <PATH>] [-c|--config <PATH>] [-j|--jobs <N>] [PATHS...]
```

类似 `inclean apply`，但不修改文件，只显示修改的 diff。

- `-o` 指定输出文件路径。如果不指定，则输出到标准输出。
- 输出格式：unified diff（`diff -u` 风格），支持多文件拼接（每个文件以标准 `--- a/PATH` / `+++ b/PATH` 头分段）。**仅包含有改动的文件**，未改动的文件不出现在输出中。
- diff 的产生不取决于是否有 unfixable 违规：可修复部分一律产出 diff；unfixable 部分依旧在末尾报错列出，并以非零状态码退出。

```sh
inclean explain # 暂时先不实现。移除所有相关代码。
```

```sh
inclean config new [PATH]
inclean init [PATH]        # alias of `inclean config new`
```

在指定路径创建一个新的 `inclean.toml` 配置文件（模板）。`PATH` 的含义：

- `PATH` 是已存在的目录 → 在该目录下创建 `inclean.toml`
- `PATH` 是已存在的文件 → 报错（不覆盖）
- `PATH` 不存在：
  - 如果路径以 `/` 结尾，或没有后缀且字面上看像目录 → 递归创建该目录，并在其中创建 `inclean.toml`
  - 否则 → 把 `PATH` 当作文件路径，递归创建其父目录后创建该文件
- `PATH` 省略 → 等价于 `PATH = "."`（在当前目录创建 `inclean.toml`）

新创建的配置文件包含一些示例规则和注释，供用户参考和修改，并填好 `[project].version` 和 `[project].min_inclean_version`（之后永不自动更新）。

```sh
inclean config check [-c|--config <PATH>]
```

`inclean check config` 的 alias，行为完全相同。

```sh
inclean config schema [-o|--output <PATH>] [--check]
```

显示配置文件的 schema（字段说明、默认值、占位符等）。

- 不带 `--check`：把 schema 写到 `-o` 指定的路径，或写到标准输出。`-o` 指向已存在的文件时直接覆盖。
- 带 `--check`：要求 `-o` 指定的文件已存在，将其内容与当前 CLI 内置的 schema 比较；一致则零退出码，不一致则非零退出码并打印 diff。该模式不修改文件。
- 如果 `-o` 指向已存在的目录，则默认文件名为 `inclean.toml.schema.json`。

## Engine

所有忽略和包含文件都由配置文件显示指定。不要自动加料。（比如不要自动应用 `.gitignore`、忽略 `build` 等）

### Include 行的识别

- `include_match` 的 glob 匹配对象：`#include` 后面去掉引号 / 尖括号后的**纯路径字符串**。例如 `#include "foo/bar.h"` 的匹配对象是 `foo/bar.h`。
- 路径分隔符：永远用 `/`（即使在 Windows 上）。
- glob 是**全字符串锚定**的（与 `globset`、`.gitignore` 行为一致）：
  - `foo.h` 只匹配字面 `foo.h`，**不**匹配 `a/foo.h`
  - 想匹配任意子路径下的 `foo.h`，写 `**/foo.h`
  - `*` 不跨 `/`；`**` 跨 `/`
  - 这一点用户很容易踩坑。`inclean.toml.schema.json` 和 `inclean config new` 模板里要把这条作为显眼注释写出来。

### Trailing comment 的定义

- 仅指**同一物理行**上、紧跟在 include 之后到行尾的"空白 + 注释"。
- 跨行 block comment（`/* …\n… */` 中间换行）**不算** trailing comment：这类情形下 trailing comment 视为不存在，引擎跳过 trailing_comment 处理；不阻塞 include 本体的 action。
- include 之后下一行的独立注释**不算** trailing comment。

### 源文件读写

- 文件编码原样保留。BOM、行尾（CRLF / LF / 混合）逐文件检测并保持。引擎不做任何统一化。
- 单个源文件解析失败（畸形预处理、未支持的语法等）→ **skip 该文件并 warn**，不中断整次 run。最终汇总报告会列出所有 skip 的文件。
- 文件被 skip 不影响其他文件的处理与退出码（除非 unfixable 违规出现在已成功处理的文件中）。

### 规则冲突

同一条 `#include` 语句可能被多个 rule 执行到 action 阶段。冲突检测的判据是**最终生成的整行文本**：把每条 rule 的 action（以及随后 trailing comment 的处理）一直应用到底，得到一条"将要写回该行的最终文本"。所有参与匹配的 rule 各自得到一条最终文本——

- 若所有 rule 的最终文本完全相同，不算 conflict（同一结果，无歧义）
- 若有任意两条不同，算 conflict，本 include 列入 unfixable

如此一来，action 类型本身（`resolve` / `replace` / `keep`）是否不同并不重要，重要的是结果文本是否一致。

冲突的错误信息至少应包含：
- 文件路径、行号、原 include 行原文
- 每条参与的 rule 的名称、其产出的最终文本
- 哪一部分不同（path 部分 / 引用形式 / trailing comment 部分）

示例：

```
error: rule conflict at src/foo.c:42
  original: #include "bar/baz.h" // legacy
  rule "bar":  #include "lib/bar/baz.h" // legacy
  rule "legacy": #include <bar/baz.h>     // IWYU pragma: export
  differs in: include path, output_form, trailing_comment
```

### 并行与输出保序

每个文件都要把匹配到的所有 rule 跑一遍才能确定结果。所以多线程的最小任务粒度是文件，不是 rule。

输出保序（文件字典序）的实现：

1. 预扫描得到全部待处理文件，按字典序排序，给每个文件一个序号 `0..N-1`
2. 工作线程拉任务、处理、把 `(idx, result)` 投入一个有界 channel
3. 一个单独的输出线程维护"下一个要输出的序号" `next`，从 channel 收到结果时：
   - 若 `result.idx == next`：直接输出并 `next += 1`，然后看缓存里有没有 `next` 对应的结果，循环消费
   - 否则：放入按 idx 排序的小堆缓存

这跟你最初的"用优先队列"方案等价，写出来只是想强调"先排序定序号、再多线程跑"这步是关键。

## Misc

先不用写 changelog，等 v1.0 后再写第一条。
