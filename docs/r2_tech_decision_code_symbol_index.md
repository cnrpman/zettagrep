# Technical Decision: Code Symbol Index

Status: Accepted  
Date: 2026-04-12

## Decision

代码索引直接用 `tree-sitter` 做 symbol 提取，不追求通用 AST。

首批只支持：

- Python
- Rust
- C
- C++
- Java
- JavaScript
- TypeScript

## Symbol Chunk

每个 symbol 产出一条统一的 `search_text`，BM25 和 vector 都吃这一份。

`search_text` 只由这些字段合成：

- `symbol_name`
- `signature_text`
- `doc_text`
- `container`
- `path`

file-local metadata 保留：

- `language`
- `kind`
- `rel_path`
- `container`
- `line_start`
- `line_end`
- 展示 snippet

## Extraction Scope

只提取高价值 definition：

- function / method
- class / struct / enum / interface / trait
- top-level variable / constant / field-like declaration

不做：

- 引用解析
- 类型推导
- LSP 级导航

## Root-Local Sharing

代码 symbol chunk 必须复用现有 root-local shared chunk owner 模型。

规则：

- shared identity 仍然是 `normalized_text`
- 同一 `.zg` 根内，相同 canonical symbol text 只 embed 一次
- rename / move 后，如果 canonical text 不变，embedding 可复用

约束：

- shared canonical text 里不能放 `path`
- 否则 rename 会破坏复用
- `path` / `container` 留在 file-local ref metadata 里

## Schema Impact

不新建独立 code DB，继续落在 `.zg/index.db`。

最小要求：

- file-local ref 可区分 `chunk_kind = text | symbol`
- symbol ref 可带：
  - `language`
  - `symbol_kind`
  - `container`

## Failure Model

单文件 AST lane 失败时：

- 只跳过该文件的 symbol extraction
- 不影响文档索引主路径

## Non-Goals

这次不做：

- 全文件代码 body 全量索引
- 跨文件引用图
- 类型系统集成
- 通用 AST schema
- 自动支持更多语言

## Follow-up Rule

以下实现都算偏离本决策：

- 把代码全文 body 当普通文档 chunk 全量塞进索引
- 为 code lane 单独做另一套 BM25 / vector 文本格式
- 把 `path` 放进 shared canonical identity
- 为了 symbol 搜索引入完整 LSP / semantic engine
