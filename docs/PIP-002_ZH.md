# PIP-002: Execute 提交

| 字段 | 值 |
|------|-----|
| PIP | 002 |
| 标题 | Execute 提交 |
| 状态 | Draft |
| 补位 | 白皮书 §2（execute 行动的可记录性） |
| 取代 | PIP-001 §12 |

---

> 本 PIP 重新定义 execute 行动的提交方式。PIP-001 §12 将执行与记录混为一谈，本 PIP 将二者分离：行动者执行，Kernel 记录。

### 关键词约定

本文中的 MUST、MUST NOT、SHALL、SHOULD、MAY 按 [RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) 解释。

---

## 动机

PIP-001 §12 原文：

> execute 行动的输入/输出 MUST 被封装为可记录结构：工作目录隔离、输出捕获、Kernel 超时熔断。

实现中将此理解为 Kernel 亲自 spawn 进程、等待完成、捕获输出。三个问题：

1. **角色混淆** — Kernel 是提交者（白皮书 §2），不是执行者。提交者提供行动的线性化提交点，不代替行动者做事。
2. **单点风险** — 长时间运行的命令阻塞 Kernel 的 submit 管道。300 秒超时是结构性负担，不是安全网。
3. **不对称** — mutate 由行动者写文件并提交元数据（content_oid），execute 却由 Kernel 执行。这种不对称没有原则性理由。

本 PIP 恢复对称性：四种 action type 遵循同一模式 — 行动者行动，Kernel 记录。

---

## 规范

**§1 行动者执行，Kernel 记录** — 行动者 MUST 自行执行并提交结果。Kernel MUST NOT 代替任何行动者 spawn、运行或等待外部进程。

**§2 Execute Payload 格式** — execute 行动的 payload MUST 包含：

| 字段 | 类型 | 说明 |
|------|------|------|
| `input_oid` | OID (`sha256:<hex>`) | 序列化输入的引用（command、args、env、工作目录） |
| `output_oid` | OID (`sha256:<hex>`) | 序列化输出的引用（stdout、stderr） |
| `exit_code` | integer | 进程退出码 |
| `artifact_hash` | string (`sha256:<hex>`) | 原始输出字节的 SHA-256 |

Kernel MUST 拒绝缺少上述任何字段或 OID 格式无效的 execute 提交。

**§3 Kernel 验证范围** — Kernel MUST 验证：

- Payload 包含全部必需字段且类型正确
- 行动者拥有足够能量和有效授权（boundary + envelope）
- OID 格式有效（`sha256:` 后跟 64 位十六进制字符）

Kernel MUST NOT 验证 OID 引用是否指向实际内容。Kernel 记录行动者声称的内容，与 mutate 行为一致。

**§4 消耗标定（修订 PIP-001 §4）** — execute 基础成本为 25。IO 附加成本根据 payload 中行动者报告的 `output_bytes` 字段计算。公式不变：execute = 25 + output_bytes / 256。

**§5 取代 PIP-001 §12** — PIP-001 §12 被本 PIP 完整取代。"工作目录隔离"、"输出捕获"、"Kernel 超时熔断"不再适用于 Kernel。执行环境管理是行动者的责任。

---

*PIP-002 Draft.*
