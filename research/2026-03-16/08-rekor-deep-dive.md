# Sigstore Rekor 深度调研

> 2026-03-16 — 接 07-timestamp-anchoring-design-space.md，专题研究 Rekor

---

## 1. Rekor 到底是什么

Rekor 是 Sigstore 项目的 **签名透明日志（Signature Transparency Log）**。本质上是三件事合一：

1. **Append-only Merkle tree** — 每个条目有 inclusion proof，根有 signed tree head
2. **REST API 服务** — 接收签名元数据，返回 log entry + 密码学证明
3. **公开可审计数据库** — 任何人可以读取全部条目、运行 monitor 检测篡改

### 与 Certificate Transparency 的关系

Rekor 直接继承了 CT log 的架构思想：
- CT log 存 TLS 证书 → Rekor 存签名事件（signature + hash + public key）
- CT 有 SCT（Signed Certificate Timestamp）→ Rekor 有 SET（Signed Entry Timestamp）
- CT 用 Merkle tree + signed tree head → Rekor 同样
- CT 用 Google Trillian → Rekor v1 也用 Trillian，v2 迁移到 Tessera（Trillian 继任者）

**关键区别：CT log 只接受证书，Rekor 接受多种类型的签名元数据。**

### Rekor 不是 TSA

| | TSA (RFC 3161) | Rekor |
|---|---|---|
| 核心功能 | 签名绑定 hash + time | 记录签名事件到 append-only log |
| 输出 | TimeStampToken（自包含） | LogEntry + inclusion proof |
| 时间来源 | TSA 签名的时间戳（密码学绑定） | 内部时钟（**非密码学绑定，可变**） |
| 验证方式 | 离线（有证书链即可） | 需要 signed tree head（可缓存但需要获取） |
| 信任假设 | TSA 私钥安全 + 时钟准确 | Log 运营方 append-only + 足够多的 monitor |

**Rekor 的时间戳是其最大的设计缺陷**：`integratedTime` 字段来自 Rekor 的内部时钟，不是 Merkle leaf 的一部分，可以被 Rekor 运营方悄悄修改而不被 Merkle tree 检测到。这就是 Sigstore 后来加 TSA 的原因。

---

## 2. Rekor 技术细节

### 2.1 API

**Rekor v1（即将淘汰）：** REST API，多个端点
- `POST /api/v1/log/entries` — 提交条目
- `GET /api/v1/log/entries/{entryUUID}` — 查询条目
- `GET /api/v1/log` — 获取 signed tree head
- `POST /api/v1/log/entries/retrieve` — 按 hash/key 搜索（v2 移除）

**Rekor v2（当前/未来）：** 简化为单一写入端点
- `POST /api/v2/log/entries` — 唯一的写入接口
- 读取通过 tile-based HTTP（CDN 可缓存）

### 2.2 提交什么

你不能提交任意 hash。你必须提交一个**签名事件**：hash + signature + public key。

Rekor v2 只支持两种类型：
- **hashedrekord** — hash + signature + public key/certificate
- **dsse** — Dead Simple Signing Envelope（SLSA attestation 格式）

v1 曾支持 intoto, rekord, helm, tuf, **rfc3161**, jar, rpm, cose, alpine — **全部在 v2 中删除**。

### 2.3 HashedRekord 提交格式（v2）

**用 Fulcio 证书：**
```json
{
  "hashedRekordRequestV002": {
    "digest": "<base64 SHA-256 digest>",
    "signature": {
      "content": "<base64 signature>",
      "verifier": {
        "x509Certificate": {
          "rawBytes": "<base64 DER-encoded certificate>"
        },
        "keyDetails": "PKIX_ECDSA_P256_SHA_256"
      }
    }
  }
}
```

**用自管理密钥（无 Fulcio/OIDC）：**
```json
{
  "hashedRekordRequestV002": {
    "digest": "<base64 SHA-256 digest>",
    "signature": {
      "content": "<base64 signature>",
      "verifier": {
        "publicKey": {
          "rawBytes": "<base64 DER-encoded public key>"
        },
        "keyDetails": "PKIX_ED25519"
      }
    }
  }
}
```

**支持的 keyDetails 包括：**
- `PKIX_ECDSA_P256_SHA_256`
- `PKIX_ED25519`（纯 Ed25519）
- `PKIX_ED25519_PH`（Ed25519ph，pre-hashed variant）

### 2.4 Ed25519 与 HashedRekord 的兼容性

**重要技术细节：** 标准 Ed25519 在签名时内部计算 SHA-512，需要原始消息来验证签名。而 hashedrekord 只存 hash，不存原始数据。

这导致两个路径：
1. **Ed25519ph（pre-hashed）**— 先 hash 再签名，hash 可独立验证。Rekor 已通过 PR #1945 添加支持。
2. **纯 Ed25519 + PKIX_ED25519** — Rekor v2 支持，因为验证时使用的是 signature + public key + digest 的组合。

**PunkGo 用的是 Ed25519，需要确认是否是 Ed25519ph。** 如果用的是标准 `ed25519::Signer`（非 pre-hash），可能需要切到 Ed25519ph 或用 ECDSA P-256。

### 2.5 Rekor 返回什么

提交成功后返回 LogEntry：

```json
{
  "body": "<base64 编码的 hashedrekord body>",
  "integratedTime": 1666309800,
  "logID": "<transparency log 的公钥 hash>",
  "logIndex": 5526036,
  "verification": {
    "inclusionProof": {
      "checkpoint": "<signed tree head / checkpoint>",
      "hashes": ["<Merkle path 中的 sibling hashes>"],
      "logIndex": 1362605,
      "rootHash": "<Merkle root>",
      "treeSize": 1362606
    },
    "signedEntryTimestamp": "<SET — Rekor 对 entry 的签名>"
  }
}
```

**关键字段：**
- `signedEntryTimestamp` (SET) — Rekor 用自己的私钥对 entry 签名，证明"Rekor 确实收到了这个条目"
- `inclusionProof` — 标准 Merkle inclusion proof，可以独立验证 entry 在 tree 中
- `integratedTime` — **不可靠**，非密码学绑定，可变

### 2.6 Curl 提交示例

```bash
# v1 API（仍可用）
curl -X POST "https://rekor.sigstore.dev/api/v1/log/entries" \
  -H "Content-Type: application/json" \
  -d '{
    "apiVersion": "0.0.1",
    "kind": "hashedrekord",
    "spec": {
      "data": {
        "hash": {
          "algorithm": "sha256",
          "value": "<hex SHA-256 hash>"
        }
      },
      "signature": {
        "content": "<base64 signature>",
        "publicKey": {
          "content": "<base64 PEM public key>"
        }
      }
    }
  }'
```

**v2 API 格式不同**（见 2.3 节），端点为 `/api/v2/log/entries`。

### 2.7 Rekor v2 架构变化

- **后端**：Trillian → Tessera（tile-based，CDN 友好）
- **延迟**：v1 同步返回，v2 批量处理，**需要等几秒钟**才能返回
- **类型**：从 9+ 种砍到 2 种（hashedrekord + dsse）
- **搜索索引**：v2 删除了搜索功能
- **见证（Witnessing）**：v2 集成第三方见证者，增强 append-only 保证
- **存储成本**：tile-based 显著降低，CDN 可缓存读取

---

## 3. Rekor vs TSA — 精确比较

### Sigstore 为什么同时用 Rekor 和 TSA

Sigstore 博客 "Trusted Time in Sigstore" 明确说明：

> Rekor 的时间戳来自内部时钟，不可外部验证，且不是 Merkle leaf 的一部分——意味着时间戳可以被悄悄修改而不被检测。

所以 Sigstore 加了 TSA 作为独立的时间源：

1. **TSA 提供：** signed(hash + time) — 密码学绑定的时间证明
2. **Rekor 提供：** 公开的 append-only 顺序证明 + SET + inclusion proof

**它们不互斥，功能互补：**

| 需求 | TSA 能满足？ | Rekor 能满足？ |
|------|------------|--------------|
| "这个 hash 在 T 时刻之前存在" | 是（签名绑定） | 否（integratedTime 可变） |
| "这个签名事件被公开记录" | 否（TST 是私有的） | 是（全球可查询） |
| "没有人能悄悄删除记录" | N/A | 是（Merkle + monitors） |
| "离线验证" | 是（自包含 TST） | 部分（需要 signed tree head） |
| "第三方可审计" | 否（需要逐个查 TST） | 是（任何人可以 mirror 和 audit） |

### 双提交模式（Sigstore 的做法）

Sigstore 的完整流程：
1. 签名 artifact
2. 向 TSA 提交 hash → 获得 TimeStampToken（时间绑定）
3. 向 Rekor 提交 signature + hash + public key → 获得 LogEntry + inclusion proof（公开记录）
4. TST 可以嵌入 Rekor entry 中（v1 的 rfc3161 type，但 v2 已删除这个 type）

**v2 的变化使双提交变复杂了**：rfc3161 entry type 被删，TST 不能直接作为 Rekor 条目提交。但 TST 仍然可以在本地保留，作为独立的时间证明。

---

## 4. Rekor 的信任模型

### 谁运营公共实例

- **URL**: `rekor.sigstore.dev`
- **运营方**: OpenSSF（Open Source Security Foundation），由 Linux Foundation 下属
- **维护者**: Google、Chainguard、GitHub、Red Hat 工程师
- **SLO**: 99.5% 可用性
- **Oncall**: 24/7 轮值
- **状态页**: https://status.sigstore.dev

### 公开可审计性

- **任何人可以读取全部条目**（这是 transparency log 的核心价值）
- **rekor-monitor**（官方项目）可以持续验证 log 的 append-only 属性
- Rekor v2 集成第三方见证者（witnessing），进一步增强信任

### 如果 Rekor 宕机

- 已提交的条目 + 本地保存的 inclusion proof → 仍可验证（如果你保留了 signed tree head）
- 无法提交新条目
- 不影响 TSA timestamps（TST 是独立的）

### 如果 Rekor 被攻破

- 攻击者修改历史 → 持有旧 signed tree head 的 monitor 会检测到不一致
- 攻击者签发虚假 SET → 不影响已有条目的 inclusion proof
- **关键：Rekor 是 tamper-evident，不是 tamper-proof** — 它不能阻止篡改，但能让篡改被发现

### 自托管

可以。Rekor 完全开源，部署方式：
- Docker Compose
- Kubernetes operator
- Helm chart
- 从源码编译

自托管的 Rekor 失去"公共可审计"的优势，但保留了 append-only Merkle 结构的密码学保证。

### 隐私

**如果用 Fulcio（OIDC keyless signing）：**
- Email 地址会出现在 Fulcio 证书的 SAN 中
- 证书进入 CT log（Fulcio 的 transparency log）
- 所有签名活动与身份公开关联 → **严重的隐私问题**

**如果用自管理密钥（BYO key）+ hashedrekord：**
- Rekor 只存：hash + signature + public key
- 不存邮件地址、不存 artifact 内容、不存元数据
- **隐私风险最小** — 观察者只知道"某个公钥签了某个 hash"
- 但公钥本身可能被关联到身份（如果在其他地方公开过）

**对 PunkGo 的影响：**
- PunkGo 提交 checkpoint root hash 的签名 → 暴露的只是 PunkGo 的 Ed25519 公钥 + root hash
- 不暴露任何 AI agent 的行为内容
- 风险可接受

---

## 5. PunkGo 使用 Rekor 的具体方案

### 5.1 提交方式

使用 `hashedrekord` 类型，自管理 Ed25519 密钥：

```
提交内容：
- digest: SHA-256(checkpoint_root_hash)  // 或者直接用 root hash 本身
- signature: Ed25519.sign(root_hash, private_key)
- publicKey: PunkGo 的 Ed25519 公钥（DER 编码）
- keyDetails: PKIX_ED25519
```

### 5.2 Ed25519 兼容性问题

**这是需要解决的技术点。**

标准 Ed25519 签名验证需要原始消息，而 hashedrekord 只存 digest。两个选项：

1. **Ed25519ph** — PunkGo 切到 pre-hashed variant。Rekor 已支持 `PKIX_ED25519_PH`。
2. **不管验证方式** — 如果 Rekor v2 的 `PKIX_ED25519` 能正确处理（因为 signature + public key + digest 一起提交），那标准 Ed25519 也行。需要实测。

### 5.3 验证方式

本地保存 Rekor 返回的：
- `signedEntryTimestamp` (SET)
- `inclusionProof`（hashes + rootHash + treeSize + checkpoint）

验证时：
1. 用 Rekor 的公钥验证 SET → 确认 Rekor 承认收到了这个条目
2. 用 inclusion proof 重建 Merkle path → 确认条目在 tree 中
3. 可选：获取最新 signed tree head → 确认 tree 仍然 consistent

### 5.4 延迟和成本

- **延迟**：v2 批量处理，预计 **2-5 秒**（比 TSA 的 ~100ms 慢很多）
- **成本**：免费（公共实例）
- **可用性**：99.5% SLO

### 5.5 Rekor 替代还是补充 TSA？

**不能替代，只能补充。**

理由：
1. Rekor 的 `integratedTime` 不可靠 → 不提供密码学时间绑定
2. Rekor v2 删除了 rfc3161 type → 不能直接存 TST
3. Rekor 宕机时无法提交 → TSA 作为独立的时间锚点仍然有价值
4. TSA 的 TST 是自包含的 → 离线验证更简单

**推荐：TSA + Rekor 双提交**
- TSA 负责：时间绑定（"这个 checkpoint 在 T 时刻之前存在"）
- Rekor 负责：公开记录（"这个 checkpoint 被写入了全球 transparency log"）

---

## 6. 核心结论

### 回答肥舅的问题："Rekor 是不是 essential？需不需要提前做？"

**不是 essential。TSA 是 Phase 1 的正确选择。Rekor 是 Phase 2 的加分项。**

理由：

1. **TSA 给的是时间绑定，Rekor 给的是公开可审计性。** 对 PunkGo Phase 1 来说，时间绑定更重要——你需要证明"checkpoint 在 T 时刻存在"，不需要证明"全世界都能看到这个 checkpoint"。

2. **Rekor 的时间戳本身不可靠。** 如果你只用 Rekor 不用 TSA，你得不到密码学时间证明。这是 Sigstore 自己承认的设计缺陷。

3. **Rekor 增加了运维依赖。** TSA 调用失败 → 你还有本地签名。Rekor 调用失败 → 你什么额外保证都没有（跟纯 TSA 失败一样）。同时依赖两个服务 = 更多失败模式。

4. **隐私可控。** 用自管理密钥 + hashedrekord，只暴露公钥和 hash，不暴露 AI 行为内容。可以接受。

5. **Ed25519 兼容性需要实测。** 不确定 PunkGo 当前的 Ed25519 签名方式是否直接兼容 Rekor 的 hashedrekord。可能需要切到 Ed25519ph。

### 什么时候加 Rekor 有意义？

当 PunkGo 需要向第三方（审计师、合规检查、企业客户）证明"这些 checkpoint 是公开可查的、不可能悄悄删除的"。这是 Phase 2 的商业化场景。

### 路线图修正

| Phase | 方案 | 保证 |
|-------|------|------|
| 1（当前）| Ed25519 签名 + RFC 3161 TSA | 身份绑定 + 时间绑定 |
| 2 | + Rekor hashedrekord | + 公开可审计 |
| 3 | + OpenTimestamps | + 物理锚定（Bitcoin） |

每一步都是上一步的严格叠加，不是替代。

### 实际的代码量估计

Rekor 集成（Phase 2）大约需要：
- HTTP POST 到 Rekor API：~30 行
- 解析 Rekor response（LogEntry + inclusion proof）：~50 行
- 本地存储 Rekor receipt：~20 行
- 验证 inclusion proof：~40 行（或复用 punkgo 已有的 tlog 验证代码）
- Ed25519 格式转换（如需 Ed25519ph）：~20 行

总计 ~160 行 Rust，不算测试。比 TSA 集成略重，但不复杂。

---

## 7. Rekor 与 PunkGo tlog 的有趣对称

PunkGo 自己就是一个本地 transparency log。Rekor 是一个公共 transparency log。

- PunkGo tlog：本地 Merkle，本地签名，append-only → 防本地篡改（模拟物理）
- Rekor：公共 Merkle，Rekor 签名，append-only → 防运营方篡改（集体监视）

把 PunkGo checkpoint 提交到 Rekor = **tlog 嵌套 tlog**。你的本地 Merkle root 成为 Rekor 全局 Merkle 的一个叶子。

这在叙事上很强："PunkGo 的物理约束，从本地延伸到了全球透明日志。"

但在实际价值上要诚实：Rekor 增加的是**检测成本**（篡改会被更多人发现），不是**物理代价**（篡改本身的成本没变）。

---

## Sources

- [Sigstore Rekor GitHub](https://github.com/sigstore/rekor)
- [Rekor v2 GA Blog Post](https://blog.sigstore.dev/rekor-v2-ga/)
- [Rekor Documentation](https://docs.sigstore.dev/logging/overview/)
- [Rekor v2 Client Guide (rekor-tiles)](https://github.com/sigstore/rekor-tiles/blob/main/CLIENTS.md)
- [Trusted Time in Sigstore](https://blog.sigstore.dev/trusted-time/)
- [Sigstore Timestamps Documentation](https://docs.sigstore.dev/cosign/verifying/timestamps/)
- [Sigstore Timestamp Authority](https://github.com/sigstore/timestamp-authority)
- [Privacy in Sigstore](https://blog.sigstore.dev/privacy-in-sigstore-57cac15af0d0/)
- [Life of a Sigstore Signature](https://www.chainguard.dev/unchained/life-of-a-sigstore-signature)
- [Rekor HashedRekord Schema](https://github.com/sigstore/rekor/blob/main/pkg/types/hashedrekord/v0.0.1/hashedrekord_v0_0_1_schema.json)
- [Ed25519ph Support PR #1945](https://github.com/sigstore/rekor/pull/1945)
- [Sigstore Transparency Log Research Dataset](https://openssf.org/blog/2025/10/15/announcing-the-sigstore-transparency-log-research-dataset/)
- [Adopting Sigstore Incrementally](https://blog.sigstore.dev/adopting-sigstore-incrementally-1b56a69b8c15/)
- [Catching Malicious Packages via Transparency Log (Trail of Bits)](https://blog.trailofbits.com/2025/12/12/catching-malicious-package-releases-using-a-transparency-log/)
