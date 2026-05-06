# zettagrep catalog vector design

状态：已确认的产品语义记录。

## 核心哲学

用户自己维护索引结构。`vector search` 的职责不是自动理解全文，而是检索用户提前写好的“目录条目”。

换句话说：

- 普通正文主要由文本检索命中。
- `*.catalog.md` 文件承载用户显式维护的目录索引。
- vector 只索引 `*.catalog.md` 中的语义命中块。
- semantic 命中后，返回对应的目录功能块，给用户后续 pointer。

## 文件约定

只有后缀为 `*.catalog.md` 的文件进入 catalog vector lane。

这个后缀的目标有两层：

- 对程序来说，它是显式 opt-in 的 catalog 文件。
- 对 LLM 来说，`catalog` 是自然语言上可理解、值得维护的文档角色。

`*.catalog.md` 必须保持通用 Markdown 格式，不引入专用 DSL。

## 条目结构

一个 catalog 条目由一个 Markdown 二级标题开始：

```md
## Retry Backoff Policy
```

每个条目里包含两个功能块：

```md
### Directory

### Search Hints
```

语义如下：

- `Directory` 是目录功能块，召回时返回给用户。
- `Search Hints` 是语义命中块，里面的 bullet 会被 vectorize。
- 两个块共同组成同一个 catalog entry。
- 两个块都可以有多个 bullets。
- `Search Hints` 中任意一个 bullet 被 vector 命中，都算命中这个 entry。
- vector 召回时返回 `Directory` 块，而不是返回命中的 hint bullet 本身。

## 自洽示例

```md
# Networking Catalog

This file is a semantic catalog for networking-related notes.
When adding, moving, renaming, or splitting related content, update these entries.

## Retry Backoff Policy

### Directory
- Primary note: [notes/network.md#retry-backoff-policy](notes/network.md#retry-backoff-policy)
- Scope: reconnect strategy, retry cap, cooldown, and jitter behavior.
- Related code: [src/net/retry.rs](src/net/retry.rs)
- See also: [notes/network.md#connection-state-machine](notes/network.md#connection-state-machine)

### Search Hints
- websocket reconnect too fast
- retry storm after disconnect
- exponential backoff with jitter
- connection keeps flapping after network loss
- how do we slow down repeated reconnect attempts

## Connection State Machine

### Directory
- Primary note: [notes/network.md#connection-state-machine](notes/network.md#connection-state-machine)
- Scope: connect, open, degraded, backoff, and closed transitions.
- Related code: [src/net/state_machine.rs](src/net/state_machine.rs)

### Search Hints
- websocket lifecycle
- reconnect state transitions
- connection phases before backing off
- degraded mode after repeated failures
- what states do we enter before retry cooldown
```

## 召回语义

用户查询进入 vector lane 时，只匹配 `*.catalog.md` 里的 `Search Hints` bullets。

如果某个 hint bullet 命中：

- 命中对象折叠到它所属的 catalog entry。
- 返回内容是同一 entry 的 `Directory` bullets。
- 可以附带说明命中了哪个 hint，但主结果仍然是 `Directory`。

如果同一 entry 的多个 hint bullets 都命中，结果仍然只返回一个 entry。

## 产品定义

`*.catalog.md` 是 entry-based semantic pointer file。

它不是普通 Markdown 全文 embedding 来源，也不是自动生成的语义索引。它是用户显式维护、LLM 也应当愿意编辑的目录文件。
