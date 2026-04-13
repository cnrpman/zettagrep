# Technical Decision: Concurrent Index Preparation

Status: Accepted  
Date: 2026-04-13

## Decision

索引构建与增量 reconcile 的并发只放在“文件读取 + 文档构建”阶段：

- 遍历候选文件后，并发执行 `load_indexable_document`
- 同一个 `.zg` 根的 writer lock 在 embedding 之前获取
- SQLite 事务写入继续保持单连接、单写者、串行提交
- shared chunk dedupe、ref_count 维护、GC 顺序保持现有语义不变
- 结果顺序必须和输入文件顺序一致，不能因为并发而改变 warning / upsert / delete 的稳定性

实现约束：

- 不引入新的并发依赖，直接使用标准库线程
- 默认 worker 数 = `min(candidate_count, available_parallelism, 8)`
- 支持用 `ZG_INDEX_THREADS` 覆盖默认 worker 数
- `ZG_INDEX_THREADS <= 0` 或无法解析时回退默认值
- 同根 writer lock 通过 SQLite `BEGIN IMMEDIATE` 获取
- writer lock 等待窗口允许扩展到 `900s`

## Why

当前索引链路里最重的纯预处理步骤是：

1. 读文件
2. UTF-8 / 文本白名单检查
3. chunk 构建
4. code symbol 提取

这些步骤彼此独立，而且发生在 SQLite 写事务之前，天然适合并发。

相反，下面这些步骤不适合在首版并发化：

- SQLite 写入
- shared chunk owner 解析与落库
- ref_count 调账
- GC

这些步骤都依赖单根内一致性的顺序语义。为了吞吐去拆成多写者，会明显放大复杂度和锁竞争风险。

## Execution Model

并发模型采用有界 worker 池：

1. 主线程先收集 candidate paths
2. 将输入路径按顺序切成若干批次
3. 每个 worker 顺序处理自己那一批文件
4. 主线程按原始输入顺序合并结果
5. 用 `BEGIN IMMEDIATE` 拿到同根 writer lock
6. writer lock 内再做 embedding 与 SQLite 写入
7. 如果另一个 `zg` 已经在同根内做 embedding，当前进程等待该 writer，而不是抢先重复计算

这里的关键不是“尽可能多线程”，而是：

- 并发只覆盖文件读取 / 文档构建
- embedding 与单写事务语义绑定在一起
- 输出稳定，可测试，可回退

## Locking Rule

同一个 `.zg` 根内，任何可能写入 shared embedding 的路径都必须先获取 writer lock，再决定哪些文本需要 embedding。

这条规则的目的不是“让写尽量快”，而是：

- 保证第二个 writer 看到的是第一个 writer 提交后的最新 `shared_chunks`
- 避免两个 `zg` 对同根内同一批缺失文本重复调用 embedding backend
- 把并发协调交给 SQLite，而不是额外引入 lockfile / pid-file

## Long-Embed Rule

embedding 可能持续数百秒，这里接受这种等待成本。

因此：

- writer lock 等待预算允许达到 `900s`
- 在等待窗口内，第二个 `zg` 应继续重试拿锁
- 超过预算后才报错

这里的取舍是：

- 优先避免重复 embedding 计算
- 接受同根写路径更长时间串行
- 不为“更快失败”牺牲 cache 一致性和总成本

## Ordering Rule

并发实现必须保持输入顺序稳定。

也就是说，给定：

```text
[a.md, b.md, c.md]
```

并发后的逻辑顺序仍然必须表现为：

```text
a -> b -> c
```

这条规则约束：

- `pending_upserts` 的构建顺序
- warnings 的首条选择
- rebuild/reconcile 的可预期性

## Non-Goals

这次不做：

- 多线程 SQLite 写入
- 每个 worker 自己持有 DB 连接并发 upsert
- 并发 shared chunk GC
- 并发 search query fanout
- 为 embedding backend 建多实例 model 池
- 通过 lockfile / pid-file 再造一套独立于 SQLite 的锁系统

## Follow-up Rule

以下任何一种实现都算偏离本决策：

- worker 直接并发写同一个 `.zg/index.db`
- 在获取同根 writer lock 之前先做向量 embedding
- 并发后输出顺序不稳定
- 为了并发重写 shared chunk / ref_count 语义
- 未经需要引入 rayon / tokio 之类新依赖
