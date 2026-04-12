# Technical Decision: Semantic Search Path

Status: Accepted  
Date: 2026-04-12

## Decision

语义检索统一采用一条默认执行路径：

- 向量索引使用 `sqlite-vec` 的 `vec0`
- 路径范围过滤放在普通 SQLite 表里做
- 过滤后的候选主键通过 `id IN (...)` / `chunk_id IN (...)` 回喂给 `vec0`

对应 SQL 形态：

```sql
SELECT chunk_id, distance
FROM vec_index
WHERE embedding MATCH :query
  AND k = :k
  AND chunk_id IN (
    SELECT c.id
    FROM chunks c
    JOIN files f ON f.id = c.file_id
    WHERE ...
  );
```

唯一保留的小特例：

- 没有任何路径过滤时，直接查 `vec0`

## Why

这条路线最简单，代码最少，也最稳定：

- 不引入 `metadata columns`
- 不引入 `partition key`
- 不为 file scope / directory scope 维护单独的 brute-force 查询分支
- 路径前缀匹配继续使用普通表最自然的表达方式
- 语义检索 planner 只需要理解“有过滤”和“无过滤”两种情况

## Non-Goals

这里不做这些特化：

- 不为路径前缀过滤设计 `vec0 metadata`
- 不把路径层级编码进 `partition key`
- 不维护 scope-specific 的 `vec_distance_cosine(...)` 全表扫描分支
- 不为了理论上的最优物理计划增加更多查询变体

## Consequences

优点：

- 查询路径统一
- 代码面更小
- 文本表上的路径过滤逻辑可以独立演进
- 后续加 ACL、tag、repo、join 过滤时仍然复用同一模式

代价：

- `chunk_id IN (...)` 不是 `partition key` 那种真正的分片裁剪
- 如果过滤候选集合非常大，性能可能不如专门建模的 `partition key`

这里接受这个代价，优先换取实现和维护复杂度下降。

## Follow-up Rule

后续新增语义检索过滤条件时，默认先问一个问题：

“能不能先在普通表里筛出 chunk/file id，再用 `IN (...)` 喂给 `vec0`？”

如果能，就不要新增特化路径。
