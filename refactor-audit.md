# inclean 重构问题清单（v0.3 spec vs 现实现）

对照源：[refactor.md](refactor.md) / [src/](src/) / [tests/](tests/) / `/home/inaku/.claude/plans/eventual-leaping-cloud.md`

## A. CLI 命令结构（与 spec 严重偏差）

| # | spec 要求 | 现实现 | 文件 |
|---|---|---|---|
| A1 | `inclean check config` / `inclean check unfixable` / `inclean check all` 三个**子命令**（位置参数） | 用 `-l/--level config\|full` 标志；**没有 `unfixable`** 一档 | [src/cli/mod.rs:40-83](src/cli/mod.rs#L40-L83) |
| A2 | `check` / `apply` / `diff` 接受 `[PATHS...]`（多个文件/目录过滤） | 只接受单个 `[DIR]`；管道也不会按这个过滤 | [src/cli/mod.rs:42-50](src/cli/mod.rs#L42-L50) [src/pipeline/run.rs:124](src/pipeline/run.rs#L124) |
| A3 | `inclean apply [-c\|--config <PATH>] [-j\|--jobs <N>] [PATHS...]` 命令选项 | 顶层 `-c` / `-j` 是 `global = true` 但全部带 `#[allow(dead_code)]`，没有任何 handler 真的消费它们 | [src/cli/mod.rs:17-27](src/cli/mod.rs#L17-L27) |
| A4 | `inclean diff [-o\|--output <PATH>]` | diff 没有 `-o`，只能输出到 stdout | [src/cli/mod.rs:47-50](src/cli/mod.rs#L47-L50) [src/cli/diff.rs](src/cli/diff.rs) |
| A5 | spec 只有 `inclean config schema`，没有顶级 `inclean schema` | 同时存在 `inclean schema` 顶层快捷方式 | [src/cli/mod.rs:53-55](src/cli/mod.rs#L53-L55) |
| A6 | `inclean config check [-c\|--config <PATH>]`（应通过 `-c` 取配置路径） | 取的是位置 `[DIR]`，没有 `-c` | [src/cli/mod.rs:93-97](src/cli/mod.rs#L93-L97) |
| A7 | `inclean check config` 应是 `inclean config check` 的等价 alias | `check --level config` 走 alias，子命令 `check config` 不存在（spec 用法 `inclean check config` 会被 clap 解为 `dir=config`） | 同上 |
| A8 | `inclean config new [PATH]` 与 `inclean init [PATH]` 是 alias | OK，但都把 PATH 默认设为 `"."`，无法区分"用户没传"和"用户传了 `.`" — spec 对二者并无明示差异，但应一致 | [src/cli/mod.rs:35-38](src/cli/mod.rs#L35-L38) [src/cli/mod.rs:99-101](src/cli/mod.rs#L99-L101) |

## B. Apply / Diff / Check 行为偏差

| # | spec 要求 | 现实现 | 文件 |
|---|---|---|---|
| B1 | apply：**fixable 部分照常应用**，文末报告 unfixable 详情 | 顶层 `if !summary.conflicts.is_empty() { bail!(...) }` — 一旦出现任何 conflict 就**完全中止**，不写入任何文件 | [src/pipeline/run.rs:204-210](src/pipeline/run.rs#L204-L210) |
| B2 | apply：报告 unfixable 时应"包括文件路径、行号、违规的 include 语句、违规类型、违规的规则名称等" | 只输出 `wrote N file(s); M file(s) skipped due to errors`，**不打印每条 unfixable 的详情** | [src/cli/apply.rs:38](src/cli/apply.rs#L38) |
| B3 | diff：unfixable 部分依旧在末尾报错列出，并以非零状态码退出 | 状态码对了，但 diff 命令不打印 unfixable 列表 | [src/cli/diff.rs](src/cli/diff.rs) |
| B4 | check：`config` 档要求"数组类字段出现用户字面写出的重复元素时报 warning（splat 展开后产生的不算）" | 完全未实现：[src/cli/check.rs::print_config_report](src/cli/check.rs#L21-L53) 只打印 rule 列表 | [src/cli/check.rs:21-53](src/cli/check.rs#L21-L53) |
| B5 | check：`unfixable` 档"只要 hit 到任何一条配置了 `action.type='error'` 或 `trailing_comment.transform.action.type='error'` 的规则，或 rule conflict，就算 unfixable" | 既无 `unfixable` 档也无对应过滤逻辑 | 整个 check.rs |
| B6 | apply 显式 "正式启动引擎前，自动先检查 config" | 当前是隐式（`run(.., Run)` 一路加载配置），没有"先 config-only check 一次再跑引擎"的两步语义；config 错误时也不会先给出 config-check 风格的友好提示 | [src/cli/apply.rs:11-12](src/cli/apply.rs#L11-L12) |

## C. 引擎 / 管道行为偏差

| # | spec 要求 | 现实现 | 文件 |
|---|---|---|---|
| C1 | "所有忽略和包含文件都由配置文件显示指定。不要自动加料。（比如不要自动应用 `.gitignore`、忽略 `build` 等）" | `WalkBuilder::new(root).standard_filters(true)` **自动**应用 `.gitignore` / `.ignore` / `.git/info/exclude`；并硬编码 skip `.git`/`target`/`node_modules`。`assert_no_extra_configs` 同样使用 standard_filters。 | [src/pipeline/run.rs:493-516](src/pipeline/run.rs#L493-L516) [src/config/discover.rs:186-196](src/config/discover.rs#L186-L196) |
| C2 | `[PATHS...]` 过滤应在 walk 之后限制处理范围 | 完全不存在过滤；`run()` 只接 `start_dir`，不接 paths 列表 | [src/pipeline/run.rs:124](src/pipeline/run.rs#L124) |
| C3 | 并行与输出保序：spec §"并行与输出保序"明确要求"按字典序定序号 + worker 投 (idx, result) channel + 输出线程维护堆缓存" | 用 `rayon par_iter().collect()`（有序输入 → 有序输出），通道+堆**未实现**。功能等价但与 plan/M5 描述不一致；尚未对接 streaming progress hook | [src/pipeline/run.rs:163-167](src/pipeline/run.rs#L163-L167) |
| C4 | conflict 错误信息应"包含…哪一部分不同（path 部分 / 引用形式 / trailing comment 部分）" | `IncludeOutcome::Conflict { rule_outputs }` 只存 `(rule_name, final_text)`，没有 `differing_aspects`；CLI 也未做 diff 分类输出 | [src/pipeline/run.rs:109](src/pipeline/run.rs#L109) [src/cli/check.rs:92-100](src/cli/check.rs#L92-L100) |
| C5 | 跨行 block comment "**不算** trailing comment …引擎跳过 trailing_comment 处理" | lex 端做对了（cross-line `/*` → 空 trailing_range + style=None），但 [action::process_trailing](src/rule/action.rs#L387-L427) 在 `style=None` 且 transform 已配置时**没有显式跳过 transform**；它会落入"transform 不匹配 → 保留 trailing"的分支，看似 OK，但若 `append_if_absent` 配了，且原 trailing 在源里是 cross-line block，`original_trailing` 是空字符串，结果会**误触发 append_if_absent** — spec 期望该 include 完全跳过 trailing 处理 | [src/rule/action.rs:387-427](src/rule/action.rs#L387-L427) |
| C6 | resolve 找不到 → "no include_directories entry contains '<text>'"；多于一处 → "include resolves under multiple include_directories: ..."；且文案应清晰 | 现实现错误信息提到的是"requires include_directories"/"found `x` in multiple include_directories"，措辞与 plan 文案不完全一致；功能上对，但 spec/plan 期望的诊断与表达不同 | [src/rule/action.rs:237-268](src/rule/action.rs#L237-L268) |
| C7 | "include_directories 是 literal 路径" 的用户决策应当落到模板/文档/schema 描述里 | schema 字段注释仍写 `pub include_directories: Option<Vec<String>>`，spec TS 注释仍写 `// glob pattern`，**没人把"literal path 不是 glob"明确写出来** | [src/config/schema.rs:78-79](src/config/schema.rs#L78-L79) [refactor.md:26](refactor.md) |
| C8 | `compile_rules` 应保留**声明顺序**（spec 多处依赖声明顺序，例如 conflict 错误里枚举 rule 的顺序） | `compile_rules` 从 `BTreeMap<String, ResolvedRule>` 取值，是字母序，**不是声明序** | [src/pipeline/run.rs:483-491](src/pipeline/run.rs#L483-L491) [src/config/copy.rs:212-218](src/config/copy.rs#L212-L218) |
| C9 | `any_rule_eligible` 用 `config_dir_relpath`（"rule 的 paths 限制在其声明所在的 config 目录之下"）做祖先判断 — 但 v0.3 禁止 sub-config，所有 rule 都在 project root 下，这段逻辑变成不必要的死代码 | 留着的话不出错，但属于遗留代码、对未来 sub-config 设计造成误导 | [src/pipeline/run.rs:518-535](src/pipeline/run.rs#L518-L535) [src/rule/engine.rs:87-92](src/rule/engine.rs#L87-L92) |
| C10 | scan 的"parse 失败 → skip + warn"应同时覆盖**畸形预处理 / 不支持的语法**等情况，不止 UTF-8 失败 | 现实现只有"非 UTF-8 → SkippedFile"。lex 出现未闭合 `"` / 未闭合 `<` 等情况是静默吞掉、不报告。最后一个 include 之后的 `#includefoo` 等 case 也无 warn | [src/pipeline/run.rs:294-313](src/pipeline/run.rs#L294-L313) [src/lex/include_line.rs:236-253](src/lex/include_line.rs#L236-L253) |
| C11 | 行尾保留：spec 要求"行尾（CRLF / LF / 混合）逐文件检测并保持。引擎不做任何统一化" | 行尾在 action 端从原行抓 `\r\n`/`\n`（OK），但**整个文件层面没有归一化检测/回写策略文档**。新插入文本（如 `comment_out`/`remove`）若文件是 CRLF 也是 OK 因从原行复制 terminator，但若 `append_if_absent` 含 `\n`，没保证转 CRLF | [src/rule/action.rs:192-207](src/rule/action.rs#L192-L207) |

## D. 配置语义 / 占位符

| # | spec 要求 | 现实现 | 文件 |
|---|---|---|---|
| D1 | `${copied}` 三种上下文中明确要求"**对象上下文**：整体替换为父值"（如 `transform = "${copied}"`） | 现实现只支持**标量字段**字符串内"${copied}"和**数组元素**splat。对象/嵌套结构的 `transform = "${copied}"` 这一形态未实现（serde 也接不住 — `RawTrailingTransform` 是结构体，不是 `String\|Object`） | [src/config/copy.rs:316-403](src/config/copy.rs#L316-L403) [src/config/schema.rs:212-218](src/config/schema.rs#L212-L218) |
| D2 | spec §"复制（copy）语义"明确写：标量/对象上下文下，若父值是 `null` 而当前字段类型不可为 null：check 时报错 | `resolve_str` 当 child=`${copied}` 且 `parent=None` 时报错。但 `parent=Some("")` 是空字符串、`relative_to`/`with` 等非空字段无空值校验 | [src/config/copy.rs:355-377](src/config/copy.rs#L355-L377) |
| D3 | `[project].root` 默认 `"."`、相对于 config 文件目录 | OK | [src/config/discover.rs:150-178](src/config/discover.rs#L150-L178) |
| D4 | `RawTrailingTransform.action` 在 TS 类型里**不是 optional**（`action: ...` 无 `?`） | 现实现是 `Option<RawTrailingAction>`；None 时悄悄退化为 `Keep { Preserve, "" }`。spec 不允许这种隐式默认 | [src/config/schema.rs:212-218](src/config/schema.rs#L212-L218) [src/config/copy.rs:621-627](src/config/copy.rs#L621-L627) |
| D5 | spec 的 `include_directories` 字段，类型 `string[]`，注释 `// glob pattern`，**但例子里全是字面路径**。用户决策已定为 literal — spec 文案需要修正 | refactor.md 的字段注释还是 `// glob pattern`，没改 | [refactor.md:26](refactor.md) |

## E. M7 测试覆盖（plan 列出的几乎全部缺失）

plan §M7 列举要保留/重写/新增的固件与测试，对照实际结果：

| plan 要求 | 状态 |
|---|---|
| 重写 `action-error` fixture | ❌ 未做（fixture 已删，无替代） |
| 重写 `angle-allowed` fixture | ❌ |
| 重写 `auto-file-dir` fixture | ❌ |
| 重写 `flat-library` fixture | ❌ |
| 重写 `multi-module-library` fixture | ❌ |
| 重写 `nested-library` fixture | ❌ |
| 重写 `out-of-tree-config` fixture | ❌ |
| 重写 `trailing-comment-policies` fixture | ❌ |
| 新增 `copy-transitive` 测试（A→B→C 链 + ${copied} splat） | ❌ |
| 新增 `conflict-by-final-text` 测试（同/异路径×output_form 等几种） | ❌ |
| 新增 `suppression-regions` 测试（USER CODE BEGIN/END + skip 标记） | ❌ |
| 新增 `cross-line-block-comment` 测试 | ❌ |
| 新增 `encoding-preservation` 测试（CRLF + BOM 端到端） | ❌（仅有 BOM 单元测试） |
| 新增 `parse-failure-skip` 测试（畸形文件 + 正常文件并存） | ❌（仅有非 UTF-8 单元测试） |
| 新增 `comment-out` 测试 | ✅ golden_tests/comment-out-action |
| 新增 `macro-form-errors` 测试 | ❌ |
| `tests/run_fixture_tests.rs` 适配新 CLI | ⚠️ 文件保留但 fixture_tests/ 只剩 `init_template`，未覆盖业务流程 |
| `tests/run_golden_tests.rs` 适配新 CLI | ⚠️ 改了 `CheckMode::Full → Run`，但只剩 `replace-action`、`comment-out-action` 两个用例 |
| `tests/support/mod.rs` 工具收敛 | ⚠️ 未审计；按 plan 应精简 |

总计：plan 列出 16 项 M7 工作，**实际完成 1 项**（comment-out golden），其余通过单元测试粗略覆盖或完全缺失。

## F. 文档 / 模板细节

| # | spec 要求 | 现实现 | 文件 |
|---|---|---|---|
| F1 | "inclean.toml.schema.json 和 inclean config new 模板里要把 glob 全字符串锚定这条作为**显眼注释**写出来" | 需要确认模板里是否够显眼，schemas/*.json 是 schemars 自动产物，没有为这条加单独描述 | [src/cli/template.inclean.toml](src/cli/template.inclean.toml) [schemas/inclean.toml.schema.json](schemas/inclean.toml.schema.json) |
| F2 | spec §"复制语义"对"对象上下文 `${copied}`"做了示例，文档里需要解释该形态 | docs/configuration.md 是否覆盖待复核 | [docs/configuration.md](docs/configuration.md) |
| F3 | `inclean schema` 顶级别名要拿掉的同时，README/docs 里如有提及也要移除 | README 的 Commands 表 / quickstart 命令是否还提到 `inclean schema` 待复核 | [README.md](README.md#L125-L126) [README.zh-CN.md](README.zh-CN.md) |
| F4 | refactor.md 第 26 行的 `// glob pattern` 注释错误（与用户决策"literal"冲突） | 文件未改 | [refactor.md:26](refactor.md) |
| F5 | README.zh-CN.md 在本次重构中**完全没有更新** | 整文件仍是旧 v0.2 描述（如果存在） | [README.zh-CN.md](README.zh-CN.md) |

## G. 死代码 / 残留

| # | 描述 | 文件 |
|---|---|---|
| G1 | `Cli.jobs` / `Cli.config` 字段 `#[allow(dead_code)]`，定义了但没人用 | [src/cli/mod.rs:17-27](src/cli/mod.rs#L17-L27) |
| G2 | `CompiledRule.config_dir_relpath` + `is_ancestor_or_self`：v0.3 禁止 sub-config，已成死代码 | [src/rule/engine.rs:87-92](src/rule/engine.rs#L87-L92) [src/pipeline/run.rs:518-535](src/pipeline/run.rs#L518-L535) |
| G3 | `Outcome` 中 `EvaluationFailure` 与 `Error` 的退出码处理在 `summary_exit_code` 里分别给 3 和 2 — spec §"checks" 列了四类违规，但当前没有 `EvaluationFailure` 对应的明确 spec 分类（应归为 unfixable，没问题） | [src/pipeline/run.rs:269-285](src/pipeline/run.rs#L269-L285) |

## H. plan 自身的遗漏

- plan §"Open implementation-time decisions"里把"默认 action"列为待定：现实现敲定为 `Keep { Preserve }`，但**未在任何用户可见文档里说明**。
- plan §M8 写"`README.zh-CN.md` 与 README 一并更新"——实际只动了 README.md。
- plan §M8 写"**Do not** add a `CHANGELOG.md` entry"——遵守了，但也没在 README 的 Status 段更新到 `0.3.0`（README.md `## Status` 段还写 `0.2.0`，前文却描述 `0.3.0` 的功能，自相矛盾）。见 [README.md:175-182](README.md#L175-L182)。

---

## 总结

按严重度归类：

**P0（spec 行为偏离 / 用户体验直接受影响）**
- A1–A7：CLI 语法与 spec 完全不一致（`check` 子命令、`PATHS...`、`-c`/`-j` 真正生效、移除 `inclean schema` 顶级别名）
- B1：apply 在出现 conflict 时整体中止，违反"修复可修复部分，最后报告 unfixable"
- B2、B3：apply/diff 末尾不报告 unfixable 详情
- B4：config check 没有"重复元素 warning"
- B5：完全没有 `unfixable` 档
- C1：`.gitignore` / `target` / `node_modules` 自动忽略违反"不要自动加料"

**P1（功能性偏离 / 边角行为不符）**
- C2：`[PATHS...]` 全链路无支持
- C5：cross-line block comment + `append_if_absent` 互动可能误触发
- C7：`include_directories` literal 决策未落到文档/schema
- D1：对象上下文 `${copied}`（如 `transform = "${copied}"`）未实现
- D4：`trailing_comment.transform.action` 缺失时不应静默默认
- C8：rule 顺序按照 BTreeMap 字母序

**P2（测试 / 文档）**
- E 全项：M7 计划里 16 项测试只完成 1 项
- F1/F2/F5：模板 glob 警告、对象 `${copied}` 文档、中文 README 未更新
- 'Status' 段 README 自相矛盾（`0.2.0` vs `0.3.0`）

**P3（清理）**
- C3：并行模型与 plan/spec 描述方法不一致（功能等价，但 plan 承诺的索引 channel + 堆缓存未做）
- C4：conflict 不带 `differing_aspects` 字段
- C9、G1、G2：死代码
