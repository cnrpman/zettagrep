# zg Tech Stacks Audit

Status: Draft  
Date: 2026-04-12

## Scope

- 本文基于当前仓库的 `Cargo.toml`、`Cargo.lock`、`cargo tree --depth 1`、`src/` 全量代码阅读，以及 2026-04-12 本地 `cargo test` 结果。
- “兼容性”判断是基于当前代码调用面、仓库产品约束、以及上游 crate 说明做出的推断。
- “维护性/主流度”证据主要来自 GitHub 和 docs.rs，链接见文末 Sources。

## Current Direct Dependencies

当前仓库现在有 12 个 direct dependencies:

| Dependency | Locked version | Repo usage | Maintenance signal | Compatibility with current repo | Verdict |
| --- | --- | --- | --- | --- | --- |
| `anyhow` | `1.0.102` | `src/lib.rs` 与 `src/main.rs`，负责应用层错误收口 | `anyhow` 是 Rust 应用层事实标准之一；docs.rs 链接到 GitHub 约 `6.4k` stars | 兼容性高。当前 repo 是单 binary CLI，很适合 `anyhow::Result` 这种边界型错误收口 | 保留 |
| `base64` | `0.22.1` | `src/search/ripgrep_backend.rs`，负责解析 `rg --json` 里的 bytes 字段 | `base64` 是 Rust 生态里的事实标准实现之一 | 兼容性高。当前只在 `rg --json` 非 UTF-8 字段兜底解析时使用，侵入性很低 | 保留 |
| `blake3` | `1.8.4` | `src/index/util.rs`，负责 `content_hash` / `text_hash` 生成 | `blake3` 是主流高性能加密 hash crate，docs.rs 与官方 GitHub 都在持续维护 | 兼容性高。当前只需要稳定、快速、低碰撞风险的内容 hash，直接契合用途 | 强烈保留 |
| `clap` | `4.6.0` | `src/main.rs`，负责顶层 CLI / 子命令 / help 生成 | `clap-rs/clap` GitHub 约 `15.8k` stars，2025-11 仍在持续发版 | 兼容性高。现在的 CLI 已经有默认入口、显式 `grep/search/index` 子命令，正适合 `clap` | 强烈保留 |
| `fastembed` | `5.13.2` | `src/index/embed.rs`，负责本地 embedding 生成，当前写死使用 `ParaphraseMLMiniLML12V2Q` 并走 fastembed/hf-hub 内建下载 | `fastembed-rs` 在 docs.rs 链接到 GitHub 约 `787` stars；docs.rs 显示 2026-02 到 2026-03 仍在连续发版 | 兼容性高。当前 repo 明确走本地 embedding，`passage:` / `query:` 前缀也和 upstream 示例一致；本地测试通过。但它的 transitive tree 很重，会带入 `ort`、`tokenizers`、`hf-hub`、`reqwest`、`image` 等 | 保留。它是当前语义检索路径的核心依赖，但要持续关注编译/体积/模型分发成本 |
| `ignore` | `0.4.25` | `src/walk.rs`、`src/index/files.rs`、`src/index/util.rs`，负责 indexed traversal 的 ripgrep-style visibility rules | 同属 `ripgrep` 生态；docs.rs 当前为 `0.4.25`；上游 `ripgrep` 约 `62.3k` stars | 兼容性非常高。你们明确要求 indexed traversal 复用 ripgrep-style visibility rules，这正是 `ignore` 的强项 | 强烈保留 |
| `regex-syntax` | `0.8.10` | `src/search/mod.rs`，负责 regex-like query 的语法分析与判定辅助 | `rust-lang/regex` GitHub 约 `3.8k` stars；`regex-syntax` 是同生态核心组件 | 兼容性高。当前只拿它做“是否像 regex”的更稳健判定，没有把整个搜索栈绑到新的正则 runtime 上 | 保留 |
| `rusqlite` | `0.32.1` | `src/index/db.rs`、`src/index/sync.rs`、`src/index/hybrid.rs`，负责所有 SQLite 访问 | docs.rs 显示 `rusqlite` 最新线在 2026-03 仍有发布，仓库链接约 `4.1k` stars | 兼容性非常高。repo 的索引模型就是 SQLite-first，且 `bundled` feature 很适合这种自己控制 DB 文件的 CLI | 强烈保留。只是当前锁在较老 minor 线，后续可计划升级 |
| `serde` | `1.0.228` | `src/index/types.rs` 里 `StateMirror` 序列化 | docs.rs 链接到 `serde-rs/serde` 约 `10.5k` stars；版本发布频率很高 | 兼容性非常高，且使用面很小很稳 | 强烈保留 |
| `serde_json` | `1.0.149` | `src/index/db.rs`，负责写 `.zg/state.json` | docs.rs 链接到 `serde-rs/json` 约 `5.5k` stars；`1.0.149` 也是近几个月发布 | 兼容性非常高，当前只做轻量 JSON mirror，没有滥用 | 强烈保留 |
| `sqlite-vec` | `0.1.9` | `src/index/db.rs`，负责注册 `sqlite3_vec_init`；`src/index/hybrid.rs` 围绕 `vec0` 查询设计语义检索 | `asg017/sqlite-vec` GitHub 约 `6.8k` stars；docs.rs 显示 2026-03 仍在活跃发版；README 明确声明它还是 pre-v1 | 兼容性高。你们已经把 schema、SQL 路径、scope filter 都围绕 `vec0` 设计，替换成本很高 | 保留，但必须把它视为“核心但有升级风险”的依赖 |
| `thiserror` | `2.0.18` | `src/lib.rs`，用于轻量自定义错误类型 | `thiserror` 是 Rust 里最常见的 error derive 之一 | 兼容性高。当前只拿它给 repo 自定义 message error 做 derive，侵入性很低 | 保留 |

## Summary Judgment

- 这批依赖整体是健康的，没有“明显冷门无人维护”的大坑。
- 最主流、最稳的几类是:
  - `rusqlite`
  - `serde`
  - `serde_json`
  - `ignore` / `regex-syntax` 所在的 ripgrep / rust-lang 生态
- 相对没那么“老牌基础设施”，但仍然是活跃维护中的，是:
  - `fastembed`
  - `sqlite-vec`
- 这次已经顺手完成了 4 个高价值替换:
  - 手写 CLI parsing -> `clap`
  - `Box<dyn Error>` 风格边界错误 -> `anyhow` / `thiserror`
  - `grep-regex` + `grep-searcher` -> `grep` facade
  - 未使用的 `regex` -> 真正需要的 `regex-syntax`
- 后续又完成了 1 个高价值替换:
  - 手写 FNV-like hash -> `blake3`
- 再后续又完成了 1 个高价值替换:
  - 库内 grep facade -> runtime `rg` subprocess backend

## Compatibility Notes

- 本地 `cargo test` 在 2026-04-12 通过，共 `46` 个测试全部通过。
- `Cargo.toml` 里声明 `edition = "2024"`、`rust-version = "1.85"`，说明当前依赖组合至少对这个 repo 的工具链约束是可工作的。
- 本地 `cargo run -- --help` 已确认新 CLI 入口可运行，help 输出保持 `zg <QUERY> [PATH]` / `zg <COMMAND>` 双入口模型。
- 真正要持续关注的兼容性风险不是 `serde` / `rusqlite` 这种成熟库，而是:
  - `fastembed` 的大体积 transitive tree
  - `sqlite-vec` 仍是 pre-v1

## Self-Written Modules That Could Be Replaced By Mainstream Deps

我把“可替换”分成两类:

- 高置信度: 这些代码更像通用基础设施，确实可以少自己写
- 中置信度: 可以替，但未必现在就值得

## Changes Landed In This Round

- `src/main.rs`
  - 用 `clap` 替掉了手写 `env::args()` 分发
  - 保留了默认入口 `zg <query> [path]`
  - 保留了 `grep` / `search` / `index *` 子命令
- `src/lib.rs`
  - 错误边界收口到 `anyhow::Result`
  - 保留了轻量 message error，但改成 `thiserror` derive
- `src/search/mod.rs`
  - 用 `regex-syntax` 替代了纯字符启发式判断
  - 新增测试锁住 `C++`、`v1.2.3` 这类 plain text 不误判
- `src/search/ripgrep_backend.rs`
  - 不再自己维护 ripgrep 搜索逻辑，改为调用 runtime `rg`
  - 运行时按 `ZG_RG_BIN` -> bundled `rg` -> `PATH` 顺序解析
  - 不再自己枚举文件；regex traversal 交给 `rg` 本身递归
- `src/index/util.rs`
  - 用 `blake3` 替掉了手写 FNV-like hash
  - 配合 schema version bump，避免旧索引和新 hash 语义混用

### Remaining High Confidence Replacements

当前没有新的高置信度“应立即替换”的基础设施轮子了。

### Medium Confidence Replacements

| Current module | Current implementation | Candidate dependency | Why it fits | Recommendation |
| --- | --- | --- | --- | --- |
| `src/index/db.rs` 的 schema version / migration 管理 | 现在是 `create_schema` + `validate_schema` + 手工 `settings.schema_version` | `rusqlite_migration` | 这个 crate 就是给 `rusqlite` 用的，docs.rs 显示 2026-02 还有 `2.4.1` 发版 | 现在 schema 还小，可以继续手写；但一旦到 v4/v5，建议切 migration crate |
| `src/index/files.rs` 的文本/二进制判断 | 现在是整文件读入后做 UTF-8 + control-char 白名单 | `content_inspector` | 这是现成的“快速猜测 text/binary”的库，理念也接近 `grep` / `git diff` | 如果以后遇到误判、性能或编码边界问题，再引入；当前不是第一优先级 |
| `src/index/files.rs` 的文件类型白名单 | 现在是手写“文档型后缀 + 少量 basename”白名单 | 直接用 `ignore` 里的 type matcher，或 `globset` | 这块现在不是难写，而是策略表达比较原始；如果以后要支持“rg 风格类型族”或更复杂规则，这类库更自然 | 不是马上要改，但这是明确可以少自己维护的一块 |

## Modules I Would Keep Custom

下面这些我不建议为了“上依赖”而上依赖:

- `src/index/hybrid.rs`
  - 这里承载的是你们产品语义: lexical + vector 并列召回、RRF 融合、scope-aware SQL。
  - 这是产品核心，不是基础设施轮子。
- `src/index/sync.rs`
  - 这里是 `.zg` 的 lazy reconcile、nested root、implicit init 语义。
  - 这些规则高度绑定你们自己的产品哲学。
- `src/index/files.rs` 的 chunking / decorator strip / normalization 规则
  - 文件类型筛选可以库化，但“怎么切 chunk、怎么做轻量清洗”更像产品策略。
- `src/index/embed.rs`
  - 这里仍然是对 `fastembed` 下载/初始化的业务封装。
  - 除非你打算彻底换 embedding runtime，否则没必要再包一层别的抽象。

## CLI Libraries: Concrete Recommendation

你提到 CLI，这里我给一个明确结论:

- 这一轮已经实际落地为 `clap`
- 如果未来极端强调体积/编译时间，才值得重新比较 `bpaf`
- 如果目标只是“维持极简 parser，不想引入大而全框架”，那才考虑 `lexopt` / `pico-args`

对 `zg` 这个仓库，我的判断是:

- 现在已经不是一个只有一个位置参数的小命令了
- 已经有默认入口、显式 `grep` / `search` / `index *` 子命令
- 后面大概率还会长出更多状态显示、索引策略、模型路径、调试类参数

当前结论不再是建议，而是已验证实现:

1. 当前选择: `clap`
2. 备选轻量路线: `bpaf`
3. 不建议回退到长期手写 `env::args()` 解析

## Recommended Action Order

1. schema 开始继续演进时接入 `rusqlite_migration`
2. 如果文本/二进制误判开始出现，再考虑 `content_inspector`

## Sources

### Current deps

- `anyhow`: [docs.rs crate page](https://docs.rs/crate/anyhow/latest)
- `base64`: [docs.rs crate page](https://docs.rs/crate/base64/latest)
- `blake3`: [docs.rs crate page](https://docs.rs/crate/blake3/latest), [GitHub repo](https://github.com/BLAKE3-team/BLAKE3)
- `clap`: [docs.rs crate page](https://docs.rs/crate/clap/latest), [GitHub repo](https://github.com/clap-rs/clap)
- `fastembed`: [docs.rs crate page](https://docs.rs/crate/fastembed/latest), [GitHub repo](https://github.com/Anush008/fastembed-rs)
- `ignore`: [docs.rs crate page](https://docs.rs/crate/ignore/0.4.25/source/), [ripgrep GitHub repo](https://github.com/BurntSushi/ripgrep)
- `regex-syntax`: [docs.rs crate page](https://docs.rs/crate/regex-syntax/latest), [GitHub repo](https://github.com/rust-lang/regex)
- `rusqlite`: [docs.rs crate page](https://docs.rs/crate/rusqlite/%3E%3D0.32%2C%20%3C1.0), [GitHub repo](https://github.com/rusqlite/rusqlite)
- `serde`: [docs.rs crate page](https://docs.rs/crate/serde/latest), [GitHub org](https://github.com/serde-rs)
- `serde_json`: [docs.rs crate page](https://docs.rs/crate/serde_json/latest), [GitHub repo](https://github.com/serde-rs/json)
- `sqlite-vec`: [docs.rs crate page](https://docs.rs/crate/sqlite-vec/latest), [GitHub repo](https://github.com/asg017/sqlite-vec)
- `thiserror`: [docs.rs crate page](https://docs.rs/thiserror)

### Replacement candidates

- `bpaf`: [docs.rs crate page](https://docs.rs/bpaf)
- `rusqlite_migration`: [docs.rs crate page](https://docs.rs/crate/rusqlite_migration/latest)
- `content_inspector`: [docs.rs crate page](https://docs.rs/content_inspector)
