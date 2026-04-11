# zg High-Level Spec

Status: Working Draft  
Date: 2026-04-11

## 一句话定义

`zg` 是一个本地优先的 filesystem query engine：

- 没有索引时，它应该像一个好用的 `regex grep`
- 有索引时，它应该像一个扎在本地文件系统上的搜索引擎

目标不是只做一种检索法，而是把 `FS` 目录变成一个可持续维护、可组合查询的本地索引底座。

## 产品哲学

- 全平台架构，`macOS first`
- `Rust` 实现，优先稳定性、可移植性、单二进制分发
- 底层统一建立在 `SQLite3` 之上
- 本地优先，索引跟着目录走，而不是先上中心化服务
- `regex` 是基础能力，不被语义检索吞掉
- v1 优先固定默认值，不急着把内部策略暴露成大量配置项
- 除了我们自己的 bm25 + semantic / hybrid recall 层之外，其它低层能力如无必要尽可能复用成熟上游，尤其是 ripgrep crates
- 保持单二进制发布；如无必要，尽可能复用 ripgrep crates
- 继续保留后端抽象边界，为未来的多后端支持留口子
- 默认工作负载是写多于搜的本地记忆型文档库
- 默认新鲜度模型是 lazy-first：写入不强制同步，搜索时按需同步当前范围
- 当前行为应可见、可解释，但不等于必须可配置
- 索引方法可以不断增加，但用户心智必须保持简单

## 用户承诺

对任意目录，`zg` 都提供一致的搜索入口，但底层走不同执行路径：

- 对未建立索引的目录：
  - 如果查询是 regex，走常规文件遍历 + `regex` 搜索
  - 如果查询不是 regex，可在当前搜索范围建立目录级本地 `.zg/`，然后执行 hybrid recall
- 对已建立索引的目录：使用最近祖先 `.zg/` 对应的本地索引执行 hybrid recall
- 如果查询是 `regex` 格式，则永远按常规 `regex` 搜索执行，不走 `FTS` 或向量检索

核心原则：

`regex` 保留 grep 语义；索引能力是加法，不是替换。

## v1 范围

先做这几个能力：

- `macOS` 上稳定工作
- 目录级索引初始化
- 在被索引目录下创建隐藏索引目录 `.zg/`
- 支持嵌套 `.zg/`
- 搜索时按最近祖先 `.zg/` 解析索引根
- 无祖先 `.zg/` 的非 regex 查询可在搜索请求内建立当前搜索范围的目录级本地索引
- 搜索前对当前范围按需刷新 dirty 内容
- 建立基于 `SQLite3` 的元数据、全文索引、向量索引
- 提供最小可用 CLI：
  - `zg`
  - `zg search`
  - `zg grep`
  - `zg index init`
  - `zg index status`
  - `zg index rebuild`

## 明确非目标

v1 不做这些：

- 云端同步或多机共享索引
- 远程 daemon / 集群索引
- 非文本文件的深度内容提取
- 自动理解所有文件格式
- 依赖启发式魔法自动判断所有 query 意图
- Windows first 或 Linux-first 的平台特化优化

## 目录模型

一个目录一旦被 `zg` 建立索引，根目录下出现隐藏目录：

`<root>/.zg/`

v1 预期至少包含：

- `.zg/index.db`
  - `SQLite3` 主数据库，也是 v1 的主要事实源
- `.zg/state.json`
  - 可选的轻量状态镜像，便于诊断；不是 authoritative config

设计原则：

- 索引数据和原目录强绑定，方便复制、删除、重建
- 用户一眼能知道这个目录是否已被 `zg` 接管索引
- 即使没有全局服务，单个目录也能自解释

## 分区与嵌套模型

- `.zg/` 是用户显式声明的索引边界，也就是用户自己定义 partition
- 允许嵌套 `.zg/`
- 父级 `.zg/` 仍然覆盖整个 subtree；子级 `.zg/` 是局部增强，不是排他替代
- 搜索时优先使用离目标路径最近的祖先 `.zg/`
- 如果没有祖先 `.zg/` 且 query 不是 regex，可在当前搜索范围建立新的目录级 `.zg/`
- 如果搜索路径是单个文件且没有祖先 `.zg/`，目录级索引根默认取该文件的父目录
- 文件变更时，所有覆盖该文件的索引都要更新
- 如果搜索使用的是父级索引且当前范围内 recall 明显差，只做 non-blocking 提示，建议用户在该范围加一层索引，并明确提示成本

## 全局组件

v1 不引入 watcher，也不引入任何常驻后台进程。

主路径采用：

- 搜索时对当前范围做 on-demand reconcile
- 发现 dirty/new/deleted 内容后，按需刷新当前范围索引
- 只刷新当前搜索范围，不在每次搜索前无界重扫整个 root
- 每个目录的实际索引数据仍写回各自的 `.zg/index.db`

## 查询模型

v1 不依赖“猜测 query 是什么意思”。

明确区分三类查询：

### 1. Regex Search

入口：

- `zg <pattern> <path>`
- 或 `zg grep <pattern> <path>`
只要有符合 regex 特征(含有\w\d .* blablabla)那就是 <pattern>

行为：

- 直接遍历文件系统
- 对文本文件做常规 `regex` 匹配
- 不依赖 `.zg/` 是否存在
- 即使目录已索引，也不走 `FTS` / vector

理由：

- 保证 grep 语义稳定
- 避免索引陈旧时产生“明明文件里有却搜不到”的错觉
- 让 `zg` 在未索引目录上仍然有明确价值

### 2. Indexed Search

入口：

- `zg <query> <path>`
- 或 `zg search <query> <path>`

只要不是落入到 regex pattern 范畴的 query 就是 query

前提：

- 目标路径有祖先 `.zg/`
- 或目标路径在本次搜索中被建立为新的目录级 `.zg/`

行为：

- 先解析最近祖先 `.zg/` 作为当前索引根
- 如果当前搜索范围内存在 dirty chunk，应先按需刷新该范围的文本索引和 embedding
- 默认执行 hybrid recall：lexical 与 embedding 信号并列参与召回
- lexical 召回不得被向量分数淹没或隐藏
- 向量也不是可有可无的 rerank 附件
- `FTS5 BM25` 与向量信号共同进入召回层

如果没有可用祖先 `.zg/` 且 query 不是 regex：

- 在当前搜索范围建立目录级本地 `.zg/`
- 如果当前路径是文件，则目录级索引根取其父目录
- 完成必要的首次建库后返回 indexed hybrid recall 结果
- 不做 blocking 交互

## 未来的查询层

v1 先不把所有索引方法一次做完，但架构必须允许以后加入：

- trigram / n-gram 索引
- AST / symbol 级索引
- tags / metadata 过滤
- reranking
- 多模态扩展

长期目标是：

同一个 CLI，按显式模式或统一 query planner 调度不同索引层。

## 同步与一致性

v1 的唯一同步路径是 on-demand reconcile。

搜索或显式索引操作发生时，系统需要：

- 发现当前范围内的 `new / changed / deleted`
- 新文件：判断是否可索引，抽取内容，写入元数据、FTS、向量
- 修改文件：重新计算内容哈希，失效旧块并重建对应索引
- 删除文件：从元数据、FTS、向量索引中删除
- 重命名：视为 delete old + index new

一致性策略：

- `zg index rebuild` 永远能从磁盘真相重建全部索引
- 缓存损坏检测走被动路线：系统不在每次启动或每次搜索前对缓存做主动完整性扫描，而是在自然接触索引时识别异常并标脏；当然，对被索引文件变更的完整性扫描依然是每次搜索前的必要步骤
- 发现 broken 迹象时，被动标记为 `dirty`
- dirty 只代表当前索引不可信，不阻止搜索；系统应提示用户重建
- 标脏后由用户决定何时重建；系统只提示，不自作主张地重跑重索引

## SQLite 角色划分

`SQLite3` 是唯一持久化事实源。

高层上拆成三部分：

- 元数据表
  - 文件路径、mtime、size、hash、索引状态
- `FTS5`
  - 面向全文检索与 `BM25`
- `sqlite-vec`
  - 面向向量近邻搜索

建议的逻辑实体：

- `files`
- `chunks`
- `fts_chunks`
- `vec_chunks`
- `index_runs`

v1 不在 spec 里锁死最终 schema，但要锁死一个原则：

同一份文件内容切块后，应同时服务于全文检索和向量检索，避免两套完全割裂的数据通路；每个 chunk 同时保留 raw text 和 normalized text。

## 文件类型与切块

v1 优先支持“文本为主”的内容：

- 代码文件
- `markdown`
- `txt`
- `json / yaml / toml`
- 轻量配置和日志文件

默认跳过：

- 明显的二进制文件
- 超大文件
- 用户显式排除的路径

索引范围判定原则：

- 先过后缀白名单
- 再过编码/字符白名单
- 不靠启发式猜测“这个文件看起来像不像文本”
- 宁可少收，也不要把噪音和脏数据灌进索引

默认遵守：

- `.gitignore`
- `.ignore`
- `.zgignore`

切块原则：

- 全文和向量共用块边界
- v1 先固定默认值，不追求把 chunking 做成大量配置项
- 默认按回车切分，并支持行内硬切分标记
- 默认行内硬切分标记为 ` :: `；索引和结果展示都去掉该标记本身
- 可对笔记常见的行首装饰符做轻量清洗，例如 `-` `*` `#`
- 当前 chunking 逻辑应可通过 `zg index status` 看见
- 不在 v1 过早追求最优 chunk strategy

## Embedding Backend

v1 锁接口，不锁具体 provider。

要求：

- indexed search 的目标路径是 hybrid recall
- backend 可以是本地模型，也可以是外部 API，但都必须通过统一抽象接入
- 搜索时如果当前范围发现 dirty chunk，应优先补齐该范围所需的 embedding，而不是长期把向量信号降级为可无
- macOS first 的分发路径优先假设本地模型随包预置，而不是首次搜索临时下载
- 模型资源优先查找 `<prefix>/share/zg/models`，并允许 `ZG_MODEL_DIR` 显式覆盖
- 如果没有预置模型，v1 直接报错；不提供 adhoc 下载 fallback

原因：

- 先把 `FS + SQLite3 + on-demand reconcile + hybrid recall` 主干打稳
- 避免把项目命运绑死在某一家模型接口上

## CLI 草图

### 建立索引

```bash
zg index init ~/code/my-repo
```

效果：

- 创建 `~/code/my-repo/.zg/`
- 初始化 `index.db`
- 开始首轮建库
- 如果检测到这会形成 overlapping index，可给出 non-blocking 提示说明成本

说明：

- `zg index init` 是显式入口，但不是唯一入口
- 非 regex 的 `zg <query> [path]` 也可以触发目录级索引创建,随后再做按需 reconcile,并对这次搜索需要的内容立即补做 embedding 后写回缓存
- 如果用户明确执行 `zg index init` / `zg index rebuild`,系统可以在该显式路径中做全量预建或重建,包括把需要的 embedding 缓存提前准备好

### 常规 regex 搜索

```bash
zg grep 'TODO|FIXME' ~/code/my-repo
```

特点：

- 不要求目录已索引
- 结果语义尽量接近 `ripgrep`

### 全文检索

```bash
zg search 'sqlite vector adapter' ~/code/my-repo
```

(can omit the path to search under current pwd)
```bash
zg search 'sqlite vector adapter'
```

特点：

- 默认解析最近祖先 `.zg/`
- 如果没有祖先 `.zg/` 且 query 不是 regex，可直接在当前搜索范围建立目录级 `.zg/` 后返回结果
- 默认走 hybrid recall

### 查看状态

```bash
zg index status ~/code/my-repo
```

至少展示：

- 当前命中的索引根
- 是否已索引
- 当前 chunking 模式
- 当前行内 marker
- 当前索引范围策略
- 最后同步时间
- 文件数 / chunk 数
- FTS / vector 是否就绪
- 是否处于 `dirty` 状态
- `dirty` 原因

## 架构原则

### 1. No Magic By Default

不要自动猜太多。

`regex` 至少在 v1 必须可被显式指定；hybrid recall 可以是 indexed search 的默认行为，但必须保持语义透明、可通过 `status` 解释。

### 1.1 Fewer Knobs In V1

第一版优先使用少量固定默认值。

- 不把所有内部策略都做成用户配置
- 只有在默认值明显不够用、且语义已经稳定时，才暴露新配置项
- 当前行为应可观察、可解释，但不等于必须可调

### 1.2 Near-Zero Migration Cost

从 `ripgrep` / `grep` / `ag` / `find` 等既有工具迁移过来的用户，不应被迫学习一套全新查询语法。

- `zg <query> [path]` 应能直接给出合理结果
- 常见 flag 的语义要尽量贴近既有习惯
- 如果无法与既有习惯对齐，宁可先不提供该 flag，也不要复用同名但不同义的 flag
- 无索引场景必须是一等公民；必要时可通过目录级索引创建,加按需 reconcile,再对这次搜索需要的内容立即补做 embedding 并写缓存来返回完整结果。与此同时,用户仍可选择显式触发全量预建。

### 2. Index Is Local State, Not Cloud State

目录的索引跟目录一起存在；不把核心价值外包给远端服务。

### 3. Regex Is The Ground Truth Escape Hatch

无论索引多复杂，用户永远可以退回最朴素的文件扫描。

### 4. One Storage Engine First

先把 `SQLite3` 这条路走通，不要过早拆成多引擎。

### 4.1 Single Binary, Abstracted Backends

- 分发形态保持单二进制
- `grep/scan` 路径优先复用 ripgrep crates
- 只有 bm25 + semantic / hybrid recall 这一层保留 `zg` 自己的产品逻辑
- 复用上游能力时仍保留后端抽象，不把主流程直接绑死到单个实现

### 5. Add Index Methods Without Rewriting The Product Story

未来可以加很多索引法，但对外故事始终是：

`zg` 是统一查询你的本地文件系统内容的工具。

## v1 成功标准

这个 spec 在 v1 层面算完成，如果可以做到：

- 在 `macOS` 上对常见源码目录稳定建立索引
- `zg grep` 在未索引目录上可直接使用
- `zg search` 在已索引目录上能稳定返回 hybrid recall 结果
- 目录文件变更后，搜索前能按需刷新当前范围索引
- 出现异常时，`zg index rebuild` 可以恢复到一致状态

## 一句话总结

先把 `zg` 做成一个坚实的本地 filesystem query substrate：regex 路径始终像 `grep`，非 regex 路径在需要时可建立本地目录级索引并返回 hybrid recall 结果；默认是在搜索命中需要的范围时立即补做并写缓存，但用户也可以显式触发全量预建，未来再往上接更多索引方法，而不是一开始就把所有野心压成一句空话。
