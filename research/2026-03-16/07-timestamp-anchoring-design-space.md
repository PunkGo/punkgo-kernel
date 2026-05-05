# Timestamp Anchoring Design Space for PunkGo

> 2026-03-16 Deep research: what IS a TSA, what are the alternatives, and which approach fits PunkGo's "physics-based trust" philosophy?

---

## 1. RFC 3161 TSA: What It Actually Is

RFC 3161 defines a **Trusted Third Party (TTP) protocol** for obtaining cryptographic timestamps.

### 工作流程

1. 客户端计算数据的 hash（数据本身不发给 TSA，保密性不受影响）
2. TSA 把 hash + 当前时间绑定，用 TSA 私钥签名，返回 TimeStampToken (TST)
3. 任何人拿着 TST + 原始数据 + TSA 公钥证书链，即可验证"这个数据在此时间之前已存在"

### 信任模型

**本质上是中心化的。** 你信任 TSA 运营方：
- 不会回溯签发（backdate）
- 时钟准确（通常同步 NTP/GPS）
- 私钥安全

如果 TSA 被攻破或消失：
- 已签发的 TST 仍可验证（只要你保留了证书链）
- 但无法再获取新的时间戳
- 如果 TSA 私钥泄露，攻击者可伪造任意时间戳

### 去中心化 TSA？

RFC 3161 本身没有去中心化机制。但可以：
- **多 TSA 交叉签名**：同一个 hash 提交给多个独立 TSA，任意一个的 TST 即可验证
- **链式时间戳**：每个新 TST 包含前一个 TST 的 hash，形成链（GitTrustedTimestamps 项目就这么做）
- 这些都是"更好的承诺"，不是"物理"

### 免费公共 TSA 列表

实际可用的免费 RFC 3161 端点（来源: [Manouchehri's gist](https://gist.github.com/Manouchehri/fd754e402d98430243455713efada710)）：

| 端点 | 信任级别 | 备注 |
|------|---------|------|
| `http://timestamp.digicert.com` | Adobe Trust List | 大厂运营 |
| `http://timestamp.sectigo.com` | Adobe Trust List | |
| `http://ts.ssl.com` | Adobe Trust List | |
| `https://freetsa.org/tsr` | 非 trust list | 开源项目 |
| `http://rfc3161.ai.moda` | 非 trust list | 自动 failover，月处理百万请求 |
| `http://timestamp.apple.com/ts01` | 非 trust list | Apple 运营 |

### Sigstore TSA

Sigstore 实现了自己的 RFC 3161 TSA 服务（[sigstore/timestamp-authority](https://github.com/sigstore/timestamp-authority)）。公共实例由 OpenSSF 运营，SLO 99.5%，由 Chainguard/GitHub/Google/Red Hat 工程师维护。**但目前文档中没有明确公开的 `timestamp.sigstore.dev` 端点 URL**——cosign 文档的示例反而用的是 `freetsa.org`。

---

## 2. GitHub 作为时间戳锚点

### 2a. Git 提交时间戳

**不可用。** Git 的 `author_date` 和 `committer_date` 是客户端自报的，随便改：

```bash
GIT_COMMITTER_DATE="2020-01-01T00:00:00" git commit --date="2020-01-01T00:00:00" -m "fake"
```

Git 的 Merkle tree（commit chain）保证的是**完整性**（内容没被改），不是**时间性**。

### 2b. GitHub 服务器时间戳

GitHub 在 push 时记录服务器端时间戳：
- `verified_at` 字段（通过 REST API 可查）
- Push event 的 webhook 时间戳
- GitHub Actions artifact 的创建时间

**信任假设：你信任 Microsoft/GitHub 不篡改服务器记录。**

### 2c. GitHub Artifact Attestation

GitHub 的 Artifact Attestation（GA since 2025-09）底层用的就是 Sigstore：
- GitHub OIDC token -> Fulcio CA -> 签名证书 -> SLSA provenance predicate
- 设计目标是 build artifact 的 supply chain security
- **不适合通用时间戳**：绑定在 GitHub Actions workflow 里，不是通用 API

### 2d. PunkGo 用 GitHub Tag 方案

具体方案：push lightweight tag `punkgo/checkpoint/<tree_size>`，tag message 包含 Merkle root hash。

**优势：**
- 用户已有 GitHub 账号
- 零额外基础设施
- `git log` 即可查看历史
- GitHub 服务器端记录 push 时间

**劣势：**
- 信任 Microsoft 不篡改（跟信任 TSA 运营方一样，但 TSA 至少有标准协议）
- 需要网络 + GitHub 可用
- 不是标准协议，第三方验证需要 GitHub API
- 没有密码学签名——GitHub 只记录了时间，没有对"hash + time"做密码学绑定
- **关键缺陷：GitHub 可以事后删除/修改 tag 的元数据**

### 2e. 先例

没有找到任何正式项目把 GitHub 本身当作 timestamp authority。找到的都是：
- OpenTimestamps 集成到 git（用 Bitcoin 锚定 git commit）
- GitTrustedTimestamps（用 RFC 3161 TSA 锚定 git commit）
- 就是说，所有严肃项目都认为 git/GitHub 本身的时间戳不够，需要外部锚定

---

## 3. 其他锚定方案

### 3a. OpenTimestamps / Bitcoin

**工作原理：**
1. 提交 hash 到 calendar server（~1秒）
2. Calendar server 聚合多个 hash 到 Merkle tree
3. Merkle root 写入 Bitcoin OP_RETURN 交易
4. 等待 Bitcoin 确认（10 分钟~数小时）
5. "升级"本地 proof 文件，包含从你的 hash 到 Bitcoin block header 的完整 Merkle path

**信任模型：** 真正的去信任（trustless）。验证只需要 Bitcoin block header，不需要信任任何第三方。

**延迟：** 初始 proof ~1秒（但此时只信任 calendar server），完整 Bitcoin 锚定 10分钟~数小时。

**成本：** 免费（calendar server 运营方承担 Bitcoin 交易费，因为一笔交易可以锚定无限多 hash）。

### 3b. Ethereum

跟 Bitcoin 类似但：
- 确认更快（~15秒 vs ~10分钟）
- 需要 gas 费（每笔交易成本波动大）
- PoS 后安全模型不同——不是能量消耗支撑，而是质押支撑
- 有学术论文研究 smart contract 时间戳，但实践中用 Bitcoin 的更多

### 3c. Sigstore Rekor（Transparency Log）

**不是 TSA，但提供类似保证。**

Rekor 是 append-only transparency log：
- 每个条目有 inclusion proof（Merkle tree）
- 有 signed tree head（log 运营方签名）
- 时间来自 Rekor 内部时钟（可变的！早期设计缺陷）
- 后来加了 RFC 3161 TSA 支持，但 TSA 和 Rekor 是两个独立组件

**关键区别：**
- TSA = 时间点证明（"这个 hash 在 T 时刻存在"）
- Rekor = 顺序证明（"这个 hash 在第 N 个条目之前被记录"）+ 公开可审计

### 3d. Certificate Transparency (CT) Logs

CT log 是 append-only Merkle tree，设计给 TLS 证书。

- 理论上可以存任意数据（Google 的 Trillian 项目把 CT 泛化了）
- 但实践中 CT log 运营方只接受证书——你不能提交任意 hash
- Signed Certificate Timestamp (SCT) 承诺在 Maximum Merge Delay 内上链
- Go 的 module proxy (`sum.golang.org`) 基于同样的 tlog 原理，但是独立实现

**结论：CT log 架构启发了 PunkGo 的 tlog 设计，但不能直接用 CT log 作为锚点。**

### 3e. 其他

| 方法 | 评估 |
|------|------|
| **IPFS** | 内容寻址（content-addressed），但没有时间保证。CID 证明内容完整性，不证明时间 |
| **Nostr** | 事件有 `created_at` 字段，但客户端自报，relay 可以拒绝但不能证明。跟 git 一样不可靠 |
| **Email (SMTP headers)** | 邮件服务器时间戳是 best-effort，可以伪造。法律上偶尔被接受，密码学上无价值 |
| **Internet Archive** | 第三方可验证，但不是为此设计的，不可控，不可靠 |

---

## 4. 信任模型比较矩阵

| 方法 | 中心化？ | 信任假设 | 延迟 | 成本 | 持久性 | 可验证方 | 密码学绑定？ |
|------|---------|---------|------|------|--------|---------|-------------|
| **RFC 3161 TSA (Sigstore/FreeTSA)** | 是（单 TSA）| TSA 运营方诚实 + 时钟准确 | ~100ms | 免费 | TST 自包含，永久可验证 | 任何人（有证书链） | 是（签名） |
| **多 TSA 交叉** | 分布式 | 至少一个 TSA 诚实 | ~100ms | 免费 | 同上 | 任何人 | 是 |
| **GitHub (push/tag)** | 是（Microsoft）| GitHub 不篡改服务器记录 | ~1s | 免费 | 依赖 GitHub 存续 | GitHub 用户 | **否** |
| **OpenTimestamps/Bitcoin** | 否（去信任）| Bitcoin PoW 安全 | 10min~hrs | 免费 | Bitcoin 存续即永久 | 任何人（有 BTC 节点）| 是（Merkle path -> block header）|
| **Ethereum** | 否（去信任）| Ethereum PoS 安全 | ~15s | Gas 费（波动）| Ethereum 存续即永久 | 任何人（有 ETH 节点）| 是 |
| **Sigstore Rekor** | 是（OpenSSF）| Rekor 运营方诚实 + append-only | ~200ms | 免费 | Rekor 存续 | 任何人（有 log 副本）| 部分（顺序证明，非时间证明）|
| **CT Log** | 是（运营方）| 不接受非证书数据 | N/A | N/A | N/A | N/A | N/A |

---

## 5. 哲学分析：物理 vs 更好的承诺

PunkGo 的核心论述：**"本地 Merkle = 模拟物理，外部锚定 = 真物理"（Landauer 原则：信息是物理的）。**

按这个标准逐一评判：

### TSA = 更好的承诺

TSA 的安全性来自**信任**（trust the operator），不来自物理代价。TSA 签发一个虚假时间戳的物理成本几乎为零——只需要私钥。这跟法律一样是规则系统，不是物理系统。

但：多 TSA 交叉签名提升了篡改成本——需要同时攻破多个独立运营方。成本从零变成了"困难但不是物理不可能"。

### GitHub = 最弱的承诺

GitHub 甚至没有对"hash + time"做密码学绑定。GitHub 的时间戳只是数据库里的一条记录。Microsoft 可以改。这是"承诺中的承诺"——信任链最长，断裂点最多。

### Bitcoin/OpenTimestamps = 接近真物理

Bitcoin PoW 的安全性来自**能量消耗**——改写历史需要重新消耗比整个网络更多的电力。这是 Landauer 原则的直接体现：信息（时间戳）被物理（能量）锚定了。

缺陷：
- 延迟太高（AI agent 的 action 是秒级的，Bitcoin 确认是小时级的）
- 实际上依赖 calendar server 做聚合——calendar server 挂了就提交不了

### Rekor = 介于承诺和物理之间

Rekor 的 append-only log 是可审计的——如果有人保留了旧的 signed tree head，就能检测到篡改。这不是物理代价，但是**检测代价**：篡改者需要同时攻破 log 和所有持有旧 tree head 的观察者。

这跟 PunkGo 自己的 tlog 其实是同一个思路——区别只是 Rekor 有更多的第三方观察者。

---

## 6. GitHub 方案详细分析

### 假设的工作流

```
1. PunkGo daemon 在 checkpoint 时：
   - 计算 Merkle root hash
   - git tag -m "<root_hash>" punkgo/checkpoint/<tree_size>
   - git push origin punkgo/checkpoint/<tree_size>
2. GitHub 服务器记录 push 时间
3. 验证者：
   - git fetch --tags
   - git log --format="%H %aI" punkgo/checkpoint/42
   - 对比 tag message 中的 root hash 与本地计算的 root hash
```

### 致命问题

1. **没有密码学时间绑定**：GitHub 的 push timestamp 是元数据，不是签名。GitHub 数据库可以改。
2. **验证依赖 GitHub**：离线验证不可能。GitHub 挂了就验不了。
3. **tag 可以被覆盖**：`git push --force origin punkgo/checkpoint/42` 可以覆盖旧 tag。
4. **没有标准协议**：每个验证者需要理解 PunkGo 的 tag 约定。

### 如果要补救

- 用 GPG/SSH signed tag -> 至少有密码学绑定（但时间仍然是自报的）
- 用 GitHub API 的 `verified_at` -> 但这只证明 GitHub 验证了签名，不证明时间

### 结论

**GitHub 不适合作为 timestamp authority。** 它可以作为一个方便的分发渠道（把 checkpoint 推到 GitHub 让别人看到），但不能替代密码学时间戳。

---

## 7. 可插拔锚定后端？

### 提议

```toml
[anchor]
backend = "tsa"  # or "rekor" or "opentimestamps" or "github" or "none"

[anchor.tsa]
url = "https://freetsa.org/tsr"
# or multiple:
# urls = ["https://freetsa.org/tsr", "http://timestamp.digicert.com"]

[anchor.opentimestamps]
calendar_urls = ["https://a.pool.opentimestamps.org"]
```

### 评估

**这是好架构，不是过度工程。** 理由：

1. **PunkGo 的 tlog 层已经跟存储层分离了**（SQLite 可换）。锚定层也应该分离。
2. **不同用户有不同需求**：
   - 个人开发者：免费 TSA 够了
   - 企业合规：可能需要 qualified TSA（EU Trust List）
   - 密码朋克：只接受 Bitcoin
   - 离线环境：none（本地 Merkle 已经比什么都没有强）
3. **PunkGo 声称"物理治理"**：如果硬编码 FreeTSA，用户会说"你就是信任一个免费服务而已"。可插拔 = 把选择权给用户 = 诚实。

### 但是——Phase 0 不该做这个

- PIP-003 的计划是先做 Ed25519 签名 + RFC 3161 TSA
- 可插拔后端是 Phase 2+ 的事
- 现在做就是 CrossGate 教训重现：铺太广，没产品

### 推荐路径

1. **Phase 1**（当前）：硬编码 `freetsa.org` 或 `timestamp.digicert.com`，一个 TSA，最简实现
2. **Phase 1.5**：抽象出 `AnchorBackend` trait，但只实现 `TsaBackend`
3. **Phase 2**：加 `RekorBackend`（Sigstore 生态整合）
4. **Phase 3**：加 `OpenTimestampsBackend`（真物理锚定，面向 crypto-native 用户）
5. **永不做**：`GitHubBackend`（不是密码学时间戳，别混淆概念）

---

## 8. 核心结论

### 对 PunkGo 的建议

1. **RFC 3161 TSA 是正确的 Phase 1 选择。** 延迟低、免费、标准协议、自包含验证。信任假设明确（trust the TSA），但 PunkGo 已经在论文中诚实披露了信任模型。

2. **多 TSA 交叉是低成本的信任提升。** 同时提交给 2-3 个独立 TSA，任意一个诚实即可。80 行代码，信任模型显著改善。

3. **OpenTimestamps 是"真物理"的终极形态。** 如果 PunkGo 要说"physics-based trust"不只是修辞，最终需要 Bitcoin 锚定。但延迟问题需要解决（可以用 TSA 做即时锚定 + OTS 做延迟确认的双层方案）。

4. **GitHub 不是 timestamp authority。** 可以作为分发渠道，不能替代密码学时间戳。

5. **可插拔后端是好架构**，但现在不是做的时候。先交付 Phase 1。

### 哲学裁决

| 方法 | 性质 |
|------|------|
| TSA | 更好的承诺（trust-based，但有标准协议和多方选择） |
| 多 TSA | 分布式承诺（single point of failure 消除） |
| Rekor | 可审计的承诺（append-only + 第三方监视） |
| Bitcoin | 物理锚定（能量消耗 = Landauer 原则的直接体现） |
| GitHub | 承诺中的承诺（最弱） |

PunkGo 的叙事应该是："Phase 1 我们用 TSA（承诺），但架构为 Bitcoin 锚定（物理）留好了接口。从承诺到物理是一条连续的光谱，不是二元对立。"

---

## Sources

- [RFC 3161 Time-Stamp Protocol](https://www.ietf.org/rfc/rfc3161.txt)
- [Trusted Timestamping - Wikipedia](https://en.wikipedia.org/wiki/Trusted_timestamping)
- [Sigstore Timestamp Authority (GitHub)](https://github.com/sigstore/timestamp-authority)
- [Trusted Time in Sigstore (Blog)](https://blog.sigstore.dev/trusted-time/)
- [Sigstore Timestamps Documentation](https://docs.sigstore.dev/cosign/verifying/timestamps/)
- [OpenTimestamps](https://opentimestamps.org/)
- [OpenTimestamps - Wikipedia](https://en.wikipedia.org/wiki/OpenTimestamps)
- [Free RFC 3161 TSA Server List](https://gist.github.com/Manouchehri/fd754e402d98430243455713efada710)
- [Running Sigstore as a Managed Service (OpenSSF)](https://openssf.org/blog/2023/10/03/running-sigstore-as-a-managed-service-a-tour-of-sigstores-public-good-instance/)
- [GitHub Artifact Attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations)
- [Understanding GitHub Artifact Attestations (Ian Lewis)](https://www.ianlewis.org/en/understanding-github-artifact-attestations)
- [Git as Tamperproof Archive with RFC 3161 (Medium)](https://medium.com/swlh/git-as-cryptographically-tamperproof-file-archive-using-chained-rfc3161-timestamps-ad15836b883)
- [GitTrustedTimestamps](https://github.com/mabuware/GitTrustedTimestamps)
- [Sigstore Rekor](https://github.com/sigstore/rekor)
- [Certificate Transparency](https://certificate.transparency.dev/howctworks/)
- [Transparent Logs for Skeptical Clients (Russ Cox)](https://research.swtch.com/tlog)
- [Blockchain Timestamping 2025 (OriginStamp)](https://originstamp.com/blog/reader/blockchain-timestamping-2025-data-integrity/en)
- [Standard-Compliant Blockchain Anchoring for Timestamp Tokens](https://www.mdpi.com/2076-3417/15/23/12722)
- [Nostr NIP-01](https://nips.nostr.com/1)
- [OpenTimestamps Git Integration](https://github.com/opentimestamps/opentimestamps-client/blob/master/doc/git-integration.md)
