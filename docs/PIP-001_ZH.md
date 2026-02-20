# PIP-001: Action

| 字段 | 值 |
|------|-----|
| PIP | 001 |
| 标题 | Action |
| 状态 | Draft |
| 补位 | 白皮书 §2（世界模型）、§3（不变量第 6 条：治理可审计） |

---

> 本 PIP 补位白皮书未具体化的三个缺口：**E 的来源**、**P 的种类**、**可写性边界的形式**。

### 关键词约定

本文中的 MUST、MUST NOT、SHALL、SHOULD、MAY 按 [RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) 解释。

---

## 第一章：能量来源

白皮书 §2 给出 E'=E-cost(A)，但未说明 E 从何而来、如何分配。

**§1 恒星亮度** — Kernel 启动时 MUST 读取本机硬件算力（INT8 TOPS），作为能量产出的物质锚定。

**§2 持续产出** — Kernel MUST 按 tick 持续产出能量，产出率由恒星亮度决定。每次产出 MUST 写入历史。

**§3 份额分配** — 每个 Actor MUST 拥有能量份额（share），按比例从产出中接收。份额在 Actor 创建时设定。

**§4 消耗标定** — 基础成本：observe=0, create=10, mutate=15, execute=25+IO。产出速率 MUST ≥ 任何基础操作成本，保障历史权不因能量设计而落空。

---

## 第二章：行动者种类

白皮书 §2 说"行动者（人或代理体）"，但未展开二者的区别与存在条件。

**§5 类型二分** — Actor MUST 为 Human（人）或 Agent（代理）之一，穷尽互斥。

**§6 Human 无条件存在** — Human Actor MUST NOT 被删除，MUST NOT 被降级。

**§7 Agent 条件存在** — Agent 创建时 MUST 声明创建者与目的。创建者 MAY 设定期限。

---

## 第三章：可写性边界

白皮书 §3 不变量第 6 条要求治理可审计，但未给出可写性边界的具体形式。

**§8 可写性声明** — 每个 Actor 创建时 MUST 声明 writable_targets（target pattern + action type）。

**§9 特权 target** — `system/*`、`ledger/*` MUST 仅 root 可写。root MAY 委托。

**§10 root 全通配** — root 初始 writable_targets MUST 为 `**`。

**§11 信封** — 临时授权 MUST 通过信封（envelope）发放：budget + targets + actions + duration + checkpoint + hold_on + hold_timeout_secs。信封权限 MUST ⊆ 创建者的 writable_targets。

> §11a **hold_on 声明** — Envelope 创建时 MAY 携带 `hold_on` 规则集（target pattern + action type）。
>
> §11b **触发** — Action 的 (target, action_type) 命中 hold_on 规则时，Kernel MUST：(1) Quote 成本；(2) Reserve 体力（锁定，防透支）；(3) 将 Action 信息 + reserved_cost 写入 hold_request 事件；(4) 返回 HoldTriggered。Envelope 保持 Active — Agent MAY 继续提交。
>
> §11c **体力锁定** — Hold 期间锁定的体力计入已用额度。Agent 可用体力 = budget - consumed - reserved。
>
> §11d **响应** — Human 通过 mutate `ledger/hold/<hold_id>` 响应：approve → 跳过 quote/reserve → 直接走 execute → settle → append；reject → settle 承诺成本 → 释放剩余 → 写 hold_response 事件。
>
> §11e **超时** — Envelope MAY 设定 hold_timeout_secs。超时等同 reject（扣部分 + 释放剩余），采用懒检查模式。
>
> §11f **reject/timeout 体力结算** — reject/timeout 时，MUST settle 承诺成本（reserved_cost 的 20%），释放剩余。承诺本身消耗了硬件资源（validate + quote + reserve + 写 event），做了功就有消耗。

**§12 execute 封装** — execute 行动的输入/输出 MUST 被封装为可记录结构：工作目录隔离、输出捕获、Kernel 超时熔断。

---

*PIP-001 Draft.*
