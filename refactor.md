# Refactor Instructions

## Config File

1. 把原来的 `@std.c_extensions` 等改为 `@c_extensions`
2. 把原来的正则表达式改为 glob pattern，使用 `globset` crate 来匹配
3. 把原来的继承逻辑改为复制逻辑，使用 `copied_from` 字段来指定要复制的规则名称。被复制的规则必须在配置文件中定义，并且必须在当前规则之前定义。复制时会把被复制规则的所有字段（包括 `name` 字段）复制到当前规则中，然后再覆盖当前规则中指定的字段。这样可以避免继承带来的复杂性和不确定性，同时也更符合用户的预期。
4. 可选字段即有默认值的字段，与必填字段相对。nullable 字段一定是默认值为 `null` 的可选字段，但可选字段不一定是 nullable 字段。

用 TS 类型定义如下（用 `?` 表示可选字段）：

```ts
type Rule = {
  name: string;
  copied_from?: string | null; // 只支持字面量（已经存在的规则名称）。Defaults to `null` (not copied).
  file_paths?: string[]; // glob pattern. Defaults to `["**/*"]` (匹配所有文件).
  file_suffixes?: string[]; // 只支持字面量和内置占位符. Defaults to `["@c_extensions", "@cpp_extensions"]`.
  suppression_comments_regex?: {
    // 匹配时按行匹配，不区分 // 和 /* */ 注释。匹配时，会把该行首尾的空白去掉再进行正则表达式匹配
    block_start?: string | null; // regex pattern. Defaults to `null` (disabled).
    block_end?: string | null; // regex pattern. Defaults to `null` (disabled).
    line?: string | null; // regex pattern. Defaults to `null` (disabled).
  };
  forms?: ("quote" | "angle" | "macro")[]; // Defaults to `["quote"]`.
  match?: string[]; // glob pattern. Defaults to `["**"]` (匹配所有 include 语句).
  include_directories?: string[]; // glob pattern
  action:
    | {
        type: "rewrite";
        relative_to: string; // 只支持字面量和内置占位符. 可用 `${the file}` 来表示当前 include 语句所在的文件路径
        form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string; // 可选。 Defaults to `""` (empty string).
      }
    | {
        type: "replace";
        with: string; // 只支持字面量和内置占位符
        form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string;
      }
    | {
        type: "keep";
        form?: "quote" | "angle" | "preserve"; // Defaults to `"preserve"`.
        message?: string;
      }
    | {
        type: "remove";
        comment_out?: boolean; // Defaults to `false`. If `true`, the include statement will be commented out instead of removed.
        keep_blank_line?: boolean; // Defaults to `false`. If `true`, the blank line will be kept after removing the include statement.
        keep_trailing_comment?: boolean; // Defaults to `true`. If `true`, the trailing comment will be kept after removing the include statement.
        message?: string;
      }
    | {
        type: "error";
        message?: string;
      };
  trailing_comment?: {
    transform?: null | {
      forms?: ("//" | "/**/")[]; // Defaults to `["//", "/**/"]`.
      match_regex: string; // 把该行首尾的空白去掉再进行正则表达式匹配. Defaults to `".*"` (匹配所有 trailing comment).
      action:
        | {
            type: "replace";
            with: string; // 只支持字面量和内置占位符
            form?: "//" | "/**/" | "preserve"; // Defaults to `"preserve"`.
            message?: string;
          }
        | {
            type: "keep";
            form?: "//" | "/**/" | "preserve"; // Defaults to `"preserve"`.
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
  - `@null`: （常量）代表一个特殊的值，表示该字段被显式设置为 null。对于可选字段，如果用户想要禁用某个功能，可以把该字段设置为 `@null`
  - `${copied}`: 代表延续被复制规则的该字段的值。比如可用 `file_suffixes = ["${copied}", ".h"]` 来表示在复制规则的基础上添加一个新的文件后缀。
- 常量：
  - `@c_extensions`
  - `@cpp_extensions`
  - `@c_header_extensions`
  - `@cpp_header_extensions`
  - `@c_source_extensions`
  - `@cpp_source_extensions`

- 路径相关：
  - `${the file}`: 代表当前 include 语句所在的文件路径
  - `${original}`: 代表原来的 trailing comment 内容（不包括注释符号和前导空格）或原来的 include 语句内容（不包括引号或尖括号）

示例：

```toml
[project]
root = "."
version = "0.3.0" # 创建该配置文件的 inclean 版本。自动生成
inclean_minimum_required_version = "0.3.0" # 解析该配置文件需要的最低 inclean 版本。自动生成
# 一次 inclean 更新有三种可能：
# 1. 完全不改 config schema 和语义
# 2. 向后兼容的改动
# 3. 向后不兼容的改动
# inclean CLI 内置记录三个版本号：
# 1. 当前 CLI 兼容的最低版本（也就是那个版本有不兼容其上一个版本的改动）
# 2. 能兼容当前 CLI 的最低版本（从那个版本到当前版本完全没有任何对 config schema 和语义的改动）
# 3. 当前 CLI 的版本

[[rule]]
name = "foo"
file_paths = ["Drivers/**"] # glob pattern
file_suffixes = ["@c_extensions"] # 只支持字面量和内置占位符
suppression_comments_regex = {
    block_start = "^USER CODE BEGIN.*$",
    block_end = "^USER CODE END.*$",
    line = "^inclean: skip$",
}
include_directories = ["Drivers/STM32F4xx_HAL_Driver/Inc", "Drivers/CMSIS/Include", "Drivers/CMSIS/Device/ST/STM32F4xx/Include"]
action = {
    type = "rewrite",
    relative_to = "${the file}",
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
    # block_start = "@null", # 禁用 block suppression comment. 此处因为 `block_start` 默认值就是 `null`，所以这里直接省略该字段即可达到同样的效果。
    # block_end = "@null", # 禁用 block suppression comment
    line = "${copied}", # 继续沿用 foo 规则的 line suppression comment regex，即 `^inclean: skip$`
}
action = {
    type = "rewrite",
    relative_to = "Drivers",
    form = "angle", # 不沿用 foo 规则的 form，而是覆盖为 "angle"
}

[[rule]]
name = "baz"
file_paths = ["cpp/include/**"] # 只限制在 cpp/include 和 cpp/src 目录下生效
file_suffixes = [".hpp", ".cpp"] # 只限制在 .hpp 和 .cpp 文件中生效
include_directories = ["cpp/include", "cpp/include/private"]
action = {
    type = "rewrite",
    relative_to = "cpp/include",
    form = "quote",
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

- `config` 只检查配置文件的语法和逻辑错误（比如必填字段缺失、继承循环、占位符错误等）。等价于 `inclean config check`。忽略传入的 `-j` 参数和 `PATHS` 参数。
- `unfixable` = `config` + 无法自动修复的违规（比如违反了 `error` 规则的违规、conflict 的问题）
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

```sh
inclean explain # 暂时先不实现。移除所有相关代码。
```

```sh
inclean config new [PATH]
```

在指定路径创建一个新的 `inclean.toml` 配置文件（模板）。如果路径不存在，则创建该路径。如果路径存在且是一个目录，则在该目录下创建 `inclean.toml` 文件。如果路径存在且是一个文件，则报错提示用户该文件已存在，无法创建新的配置文件。新创建的配置文件包含一些示例规则和注释，供用户参考和修改。

```sh
inclean config check [-c|--config <PATH>]
```

检查配置文件的语法和逻辑错误。
是 `inclean check config` 的实际实现。

```sh
inclean config schema [-o|--output <PATH>]
```

显示配置文件的 schema（字段说明、默认值、占位符等）。如果指定了输出路径，则把 schema 输出到该路径的文件中；否则输出到标准输出。
如果 `<PATH>` 已存在，则不修改文件：和该文件进行比较，如果内容相同并以零状态码成功退出；否则报错退出。
如果 `<PATH>` 是已存在的目录路径，则默认文件名为 `inclean.toml.schema.json`.

```sh
inclean init [PATH]
```

由 `inclean config new` 的 alias

## Engine

所有忽略和包含文件都由配置文件显示指定。不要自动加料。（比如不要自动应用 `.gitignore`、忽略 `build` 等）

规则冲突：
同一条 `#include` 语句可能被多个 rule 执行到 action 阶段。此时验证 action 结果是否相同，如果不相同才是 conflict.
如果相同 action 后，还有 trailing comment，则再比较 trailing comment 的处理结果是否相同.

由上述要求可知，每个文件都要用匹配到该文件的所有 rule 执行一遍，后才能确保结果。所以多线程的最小任务粒度应该是文件，而不是 rule。

此外，要求输出保序（文件字典序）。
一个解决方案是：每个任务需要知道自身输出次序，用一个优先队列存待输出结果，每次一个线程完成任务后，把结果放到优先队列里，并检查队首的结果是否是下一个要输出的，如果是则输出并更新下一个要输出的次序，直到队首的结果不是下一个要输出的.
