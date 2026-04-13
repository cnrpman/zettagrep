# Technical Decision: Shared Chunk Embeddings

Status: Accepted  
Date: 2026-04-12

## Decision

向量缓存从“file-local chunk 一份”改成“单个 `.zg` 根内，按 normalized text 共享一份”。

硬规则：

- 共享范围只在单个 `.zg` 根内
- 共享 identity 是 `normalized_text`
- provider 是 `.zg` 根级别不变量,不参与 per-chunk identity
- hash 只做加速，不能只靠 hash 判等
- 最后一个引用消失时，必须删除对应 embedding

## Why

当前实现的问题：

- 改一行会导致该文件全部 chunk 重新 embedding
- rename / move 后，即使文本没变，也会重新 embedding
- 同一 `.zg` 根内重复文本会重复存多份向量

这和本地 cache 的目标不一致。真正值得缓存的是“同根、同 normalized text”的 embedding 结果。

## Schema

目标结构：

1. `files`
   继续保存 file-level 元数据。

2. `shared_chunks`
   每个 `.zg` 根内，相同 normalized text 只保留一条 embedding owner。

   最少字段：
   - `id`
   - `normalized_text_hash`
   - `normalized_text`
   - `ref_count`

   约束：
   - `UNIQUE(normalized_text_hash, normalized_text)`

3. `chunk_refs`
   每个文件出现一次 chunk，就有一条 ref。

   最少字段：
   - `id`
   - `file_id`
   - `shared_chunk_id`
   - `chunk_index`
   - `line_start`
   - `line_end`
    - `normalized_text`
    - `normalized_text_hash`

4. `vec_chunks` / `vec_index`
   改为挂在 `shared_chunk_id` 上，不再挂在 file-local chunk 上。

5. `fts_chunks`
   继续挂在 `chunk_refs` 上，不共享去重。

## Why FTS Does Not Share

FTS / BM25 必须保留 occurrence。

同一段 normalized text 出现在多个文件里：

- 向量可以共享
- lexical posting 不能共享

否则会损坏 BM25 语义和结果展示。

## Write Path

单次 reconcile 的顺序必须是：

1. 对 dirty/new file 重新 chunk
2. 对每个 chunk 计算 `normalized_text`
3. 对 `normalized_text` 计算 hash
4. 获取同一个 `.zg` 根的 writer lock
5. 在 writer lock 内以 `(hash, normalized_text)` 查最新的 `shared_chunks`
6. 只对仍未命中的 normalized text 批量 embedding，并写入 `shared_chunks` + `vec_*`
7. 写新的 `chunk_refs`
8. 删除旧 `chunk_refs`
9. 最后统一 GC `ref_count = 0` 的 `shared_chunks`

关键点：

- hash 必须对 `normalized_text` 算
- 一次 reconcile 批次里，相同 normalized text 只 embed 一次
- 并发 `zg` 命中同一个 `.zg` 根时，第二个 writer 必须等锁，而不是在锁外先重复 embed
- GC 必须放在 transaction 末尾

## Concurrency Rule

同一个 `.zg` 根内，shared embedding 的缺失判定必须发生在 writer lock 内。

原因是：

- 如果先在锁外判断 missing，再去 embed，两个 `zg` 可能对同一批缺失文本重复计算
- 正确行为是第二个 `zg` 先等待第一个 writer 提交
- 等第一个 writer 提交后，第二个 `zg` 再读 `shared_chunks`，通常会直接复用已有 embedding

等待 writer lock 最长可以持续 `900s`，因为本地 embedding 本身可能是长操作。

## Why GC Must Be Deferred

如果边删旧 ref 边 GC，会把 rename / move 场景做坏：

- 旧路径删掉
- `ref_count` 先变 0
- embedding 被删
- 新路径再插入时又得重新 embed

正确做法是：

- 先写新 ref
- 再删旧 ref
- 最后统一 GC

## Reference Accounting

`ref_count` 是优化字段，不是唯一真相。

必须维持：

```text
shared_chunks.ref_count
==
COUNT(chunk_refs WHERE chunk_refs.shared_chunk_id = shared_chunks.id)
```

因此：

- 正常写路径维护 `ref_count`
- `rebuild` 必须能从 `chunk_refs` 重新推导并修复它

## Query Path

### Lexical

继续查 `fts_chunks` / `chunk_refs`。

### Vector

先查 `vec_index(shared_chunk_id)`，再 join 回 `chunk_refs` / `files`：

```sql
WITH knn_matches AS (
  SELECT shared_chunk_id, distance
  FROM vec_index
  WHERE embedding MATCH :query
    AND k = :k
)
SELECT ...
FROM knn_matches km
JOIN chunk_refs cr ON cr.shared_chunk_id = km.shared_chunk_id
JOIN files f ON f.id = cr.file_id
WHERE ...
```

一个 shared embedding 展开成多个 file-local hits 是正确行为。

## Display

结果展示文本不再要求存进 `chunk_refs`。

展示路径采用：

1. 搜索先返回 `rel_path + chunk_index + line range`
2. 结果物化阶段按文件分组回读文件
3. 重新 chunk
4. 用 `chunk_index` 取真实 snippet

如果文件当前不可读或 chunk 重建失败，则允许 snippet 为空。

## Collision Rule

不能只按 hash 认定命中。

必须：

- 先按 `normalized_text_hash` 缩小候选
- 再比较 `normalized_text`

## Provider Rule

provider 是 `.zg` 根级别不变量，不是 per-chunk 字段。

也就是说：

- `settings.vector_provider` 定义当前根的 active provider
- `shared_chunks` 不存 provider
- `vec_chunks` 不存 provider
- 用户切换 provider 时，必须 rebuild

## Rollout

不做 in-place schema migration。

采用的 rollout 方式是：

1. bump schema version
2. 新代码只认识新 schema
3. 旧 `.zg` 根通过 `zg index delete <path>` 或 `zg index rebuild <path>` 进入新 schema

也就是说：

- 不引入 migration framework
- 不写旧表到新表的搬运逻辑
- 不为旧 schema 保留兼容写路径

## Non-Goals

这次不做：

- 跨 `.zg` 根共享 embedding
- 近似重复文本去重
- lexical posting 去重
- raw text identity 共享
- schema migration

## Follow-up Rule

以下任何一种实现都算偏离本决策：

- embedding 仍然挂在 file-local chunk 上
- hash 不是基于 `normalized_text`
- 只按 hash 命中，不比对 `normalized_text`
- 最后一个 ref 删除后，不删除 embedding owner
- lexical posting 跟着 embedding 一起去重
