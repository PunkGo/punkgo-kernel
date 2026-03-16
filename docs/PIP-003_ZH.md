# PIP-003: 检查点认证

> 状态：**已实现**（kernel v0.5.0）
> 填充：白皮书 §3，不变量 5（可证明的仅追加性）

## 摘要

PIP-003 为 Merkle 检查点添加密码学身份绑定。每个检查点使用本地生成的 Ed25519 密钥对签名，嵌入为 C2SP 扩展行。这是三层信任架构的第一层。

## 动机

Merkle 树（PIP-001）保证仅追加完整性：任何事件不能被静默删除或修改。但如果没有签名，任何能访问数据库的人都可以从零重建整棵树并声称其为真实数据。PIP-003 添加了身份绑定：你现在可以验证*哪个 kernel 实例*产生了某个检查点。

## 规范

### §1 签名密钥生命周期

首次启动时，kernel 生成一个 Ed25519 密钥对，将 32 字节种子存储在 `{state_dir}/signing_key`。后续启动加载已有密钥。

- 密钥格式：Ed25519（RFC 8032），32 字节种子
- 存储：明文文件，位于 `{state_dir}/signing_key`
- 生成：首次启动自动生成，无需用户干预

### §2 检查点签名

每个 C2SP 检查点包含一个签名扩展行：

```
punkgo/kernel
{tree_size}
{root_hash}

sig/ed25519:{pubkey_hex}:{signature_hex}
```

签名覆盖检查点主体（origin + tree_size + root_hash，换行分隔）。扩展行格式兼容 C2SP tlog-checkpoint 和 Go 的 `sum.golang.org`。

### §3 验证

第三方验证只需要公钥和检查点文本：

```rust
verify_checkpoint_signature(pubkey_hex, checkpoint_body, sig_hex) -> bool
```

`signing_pubkey` IPC 查询类型暴露 kernel 当前的公钥。

### §4 IPC 接口

新增查询类型：

```json
{ "kind": "signing_pubkey" }
→ { "pubkey_hex": "...", "algorithm": "ed25519" }
```

### §5 信任模型

Ed25519 签名提供**身份绑定**，而非防篡改。

| 攻击方式 | 是否防御？ | 原因 |
|---------|-----------|------|
| 静默修改事件 | 是 | Merkle 树可检测 |
| 第三方伪造检查点 | 是 | 需要私钥 |
| root 操作者重建数据库 | **否** | root 持有签名密钥 |
| 回溯签发检查点 | **否** | 需要时间绑定（TSA） |

诚实披露局限：持有数据库访问权限*和*签名密钥的 root 操作者可以重建历史。这是单机信任边界——对本地主权足够（你信任自己的硬件），但不足以支撑无信任的第三方审计。

### §6 信任层架构

PIP-003 建立了三层信任架构的前两层：

| 层级 | 机制 | 提供 | 状态 |
|------|------|------|------|
| Merkle | tlog / RFC 6962 | 排序 + 完整性 | 已实现 (v0.1.0) |
| Ed25519 | 检查点签名 | 身份绑定 | **已实现 (v0.5.0)** |
| TSA | RFC 3161 时间戳 | 时间绑定 | 在 punkgo-jack 中实现 |

每一层严格叠加——没有层级替代另一层级。

未来层级（PunkGo Pool 集体见证）将扩展此架构，不修改已有层级。

## 依赖

- `ed25519-dalek` v2（含 `rand_core`）
- `rand` v0.8
- `hex` v0.4

## 向后兼容性

- 已有检查点（v0.5.0 之前）仍然有效；它们只是缺少签名扩展行
- 签名密钥自动生成；无需迁移或用户操作
- `CREATE TABLE IF NOT EXISTS audit_tsa_tokens` 为 jack 侧 TSA 存储预留前向兼容
