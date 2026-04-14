# zg Product Philosophy

文件地位: Product Philosophy (由主管亲自定义) > High-Level Spec (由主管助理定义) > Implementation (由工程师定义和执行)

## 定位与工作负载

- `zg` 首先是搜索，不是索引管理工具。
- 默认入口必须是 `zg <query> [path]`。
- 适合工作负载是假定为写多于搜的本地记忆型文档库,尤其是 markdown / note 类内容。
- CLI 心智必须小;复杂度只能留在内部,不能甩给用户。

## 交互与版本策略

- 不做 blocking 交互;只做 non-blocking 提示,而且放在结果之后。
- 第一版代码优先简单,默认值优先于可配置项。
- 如果一个行为还没有稳定到值得暴露给用户,就先固定,不急着做成配置。对用户的承诺是可见(`status` 里能看到当前逻辑),不是可配置。

## 搜索语义

- regex 是地面真相;像 regex 的输入永远按 regex 语义执行。
- 非 regex 搜索要求祖先链上已经存在显式建立的 `.zg`;如果不存在,系统应直接返回可执行的错误,提示用户先运行 `zg index init`。
- indexed search 只有两个 level:`fts` 与 `fts+vector`。
- 非 regex 搜索的 text channel 不只来自数据库 lexical index;它还应并入 ripgrep fixed-string literal recall,保证用户已经习惯的字面命中在 indexed search 里仍然可见。
- non-regex plain query 的 `r` 通道应继承 plain query 的大小写语义:默认大小写不敏感,不能因为底层用了 ripgrep literal recall 就突然退化成大小写敏感。
- regex 仍然是独立路径;plain query 结果里不混入 regex recall,只有 index text / partial literal / semantic 三类来源。
- `fts` 是 text-only 路径,用于低延迟、低成本的默认索引;这里的 text 指 indexed lexical recall 与 ripgrep literal recall 的并集。
- `fts+vector` 是 hybrid recall:text 与 embedding 信号并列参与召回,text 召回不得被向量分数淹没或隐藏,向量也不是可有可无的 rerank 附件。具体融合方式属于内部实现,可以演进,但这条语义承诺不能退化。
- 展示层应区分 `f`(来自 index lexical / FTS)、`r`(来自 ripgrep fixed-string literal recall)、`v`(来自向量召回);如果一个结果同时吃到多个来源,标签应明确表达组合关系,例如 `[rfv]`,不能把不同来源混成一个模糊标签。
- level 是显式索引决策的一部分,而不是搜索时偷偷升级的内部细节。用户建什么 level,搜索就走什么 level。
- 当结果里已经存在 `text` 或 `text+semantic` 命中时,`semantic-only` 结果只是尾部补充,当前固定最多显示 5 条;如果完全没有 text 命中,则允许 semantic 结果填满结果页。

## 新鲜度模型

- 新鲜度模型是 lazy-first:写入时不强制立即维护索引,搜索时再按需对当前范围做 reconcile。
- 搜索是同步边界:当前范围的索引正确性在 query 时刻成立,优先于"后台永远保持最新"这种做不到的承诺。
- on-demand reconcile 只应刷新当前搜索范围内的 dirty/new/deleted 内容,不应在每次搜索前无界重扫整个库。
- 对 `fts+vector` 索引,如果当前搜索范围内存在 dirty chunk 或缺失 embedding 的内容,应在这次搜索里立刻补做该范围需要的 embedding,并写回缓存,再执行 hybrid recall。
- 对 `fts+vector` 索引,同一个 `.zg` 根上的并发 reconcile/search 必须在 embedding 前串行化写者。宁可等待已有 writer 完成,也不要让两个 `zg` 对同一根里的同一批缺失向量做重复计算。
- 长时间 embedding 是可以接受的内部等待来源。等待 writer lock 可以持续数分钟,但这必须是内部恢复/协调语义,不是交给用户处理的交互流程。
- 对 `fts` 索引,默认不触发 embedding 构建。
- 当前版本不实现 watcher。没有 watcher 不等于退回到"只有 scan 才能保证正确";正确做法是在已有显式索引上由搜索请求本身触发按需 reconcile。如果以后引入 watcher/daemon,应是每用户一个后台服务,而不是每目录一个进程,并且它的角色只是让按需 reconcile 的代价更小,不是接管正确性。

## 索引边界与 partition

- local-first 必须是可见本地状态:`<root>/.zg/`。
- `.zg` 是可见的本地索引边界,也就是用户最终能观察和管理的 partition。它只能由用户显式建立。
- embedding provider 是 `.zg` 根级别的不变量,不是 per-chunk 配置。同一个 `.zg` 根在任一时刻只允许一个 active provider;如果要切换 provider,必须通过 `zg index rebuild <path>` 重建,而不是在旧索引上做混合兼容。
- index level 是 `.zg` 根级别的不变量。`fts` 根不会偷偷长出向量数据;如果要切到 `fts+vector`,必须显式 rebuild。
- 允许嵌套 `.zg`;父级索引仍可覆盖子树,子级索引是局部增强,不是排他替代。
- overlapping `.zg` 允许存在;愿意这样做的用户自己承担重复索引和重复更新的代价。
- 文件变更时,所有覆盖该文件的索引都要更新。
- 搜索使用离目标路径最近的祖先 `.zg`;如果整条祖先链上都找不到 `.zg`,则直接报错并提示用户显式建索引。
- 目前我们用 brute-force,所以这里没有额外的索引边界策略需求。
- 如果以后引入 HNSW 等 ANN,则需要重新考虑索引边界问题;到那时才需要讨论这些策略,而不是提前固化到当前实现里:
    - 经常搜索的入口目录应该单独建索引;不常搜的范围先复用父级索引。
    - 当搜索使用的是父级索引且当前范围内 recall 明显差时,只做 non-blocking 提示,建议用户在该范围加一层索引。
    - 提示必须同时说明代价:更多磁盘占用、重复索引、文件变更时更慢,而且依然不保证一定能搜到。

## 索引范围与内容处理

- 索引范围必须由后缀白名单和编码/字符白名单共同决定,不做模糊猜测。
- symlink 不参与索引遍历。
- 当前 chunking 先使用固定默认值;用户应能在 `status` 中看见当前逻辑,但不追求把它做成一堆配置项。
- 默认 chunking 按回车切分,并支持行内硬切分标记。
- 默认行内硬切分标记是 ` :: `;索引和结果展示都不保留该标记本身。
- chunk 存储同时保留 raw text 和 normalized text。
- 对笔记常见的行首装饰符可做轻量清洗,但不能破坏正文语义。

## 损坏检测策略

- 缓存损坏检测走被动路线:系统不在每次启动或每次搜索前对缓存做主动完整性扫描,而是在自然接触索引时识别异常并标脏;当然,对被索引文件变更的完整性扫描依然是每次搜索前的必要步骤。这是为了让常规路径保持低延迟,代价是某些问题要等到被触碰才会暴露——这是刻意的取舍,不是疏忽。
- 以下信号一旦被动发现,就应将索引标记为 dirty:schema/version 不匹配、数据库不可读或无法打开、必需表缺失、上次索引/重建异常中断。
- 标脏后由用户决定何时重建;系统只提示,不自作主张地重跑重索引。

## 诊断

- `zg index status` 必须是人类可读的诊断面板,至少展示:当前索引根、是否已索引、index level、chunking 模式、行内 marker、索引范围白名单策略、dirty 状态与原因、文件数、chunk 数、最后同步时间。
- 需要提供显式删除本地缓存的命令,用于删除某个目录下的 `.zg/`。

## 兼容与迁移

- 打铁还需自身硬:产品质量是一切增长的前提,迁移成本低的前提是 zg 本身值得被迁移过来
- 迁移成本必须接近零:从 ripgrep / grep / ag / find 等既有工具过来的用户,不需要学习新的查询语法就能用 `zg <query> [path]` 得到合理结果。
- 常见 flag 的语义要尽量贴合既有习惯;如果一个 flag 在 ripgrep 里意味着 X,zg 的同名 flag 不应意味着非 X。无法对齐时,宁可不提供该 flag,也不要提供一个语义冲突的同名 flag。
- regex 是地面真相这一条本身就是迁移承诺的一部分:它保证从 grep 系工具过来的用户,最熟悉的那条路径在 zg 里永远可用、永远按预期工作。
- 无索引场景里,regex 仍然必须是一等公民。对非 regex 搜索,系统可以要求用户先显式执行 `zg index init`;关键是错误必须直接、可执行、低歧义。
