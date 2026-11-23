# 打包与签名（macOS）

LaterKey 以**签名的 .app** 分发，LaunchAgent 常驻。签名方式沿用 LaterScreen：**自签名根证书**——TCC（辅助功能授权）绑定的是证书身份，所以**跨版本升级不用重新授权**；ad-hoc 签名则每次升级都要重新授权一次。

## 一次性：生成并导入签名证书

```bash
mkdir -p /tmp/laterkey-cert && cd /tmp/laterkey-cert
cat > cert.conf <<'EOF'
[ req ]
distinguished_name = dn
x509_extensions = v3
prompt = no
[ dn ]
CN = LaterKey
O = ailater
[ v3 ]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,digitalSignature
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
EOF
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 3650 -nodes -config cert.conf

security import key.pem  -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
security import cert.pem -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
security add-trusted-cert -r trustRoot -p codeSign -k ~/Library/Keychains/login.keychain-db cert.pem

security find-identity -v -p codesigning | grep LaterKey
```

把 `key.pem` / `cert.pem` 保存到安全的地方（CI 里作为 secret 注入），然后从 /tmp 删掉。

## 打包

```bash
# 本机架构，出 dist/LaterKey.app（用 LaterKey 证书签）
LATERKEY_SIGN_IDENTITY=LaterKey bash scripts/package-macos.sh

# 打包 + 装到 /Applications 并注册 LaunchAgent
LATERKEY_SIGN_IDENTITY=LaterKey bash scripts/package-macos.sh --install

# universal 胖包 + dmg
LATERKEY_SIGN_IDENTITY=LaterKey bash scripts/package-macos.sh --universal
```

不设置 `LATERKEY_SIGN_IDENTITY` 时退回 ad-hoc（`codesign -s -`），同版本内稳定、升级需重新授权。

## 首次授权

launchd 起的进程**不继承**任何应用的 TCC 授权。第一次安装后必须手动：

1. 系统设置 › 隐私与安全性 › 辅助功能；
2. 点「+」，选中 `/Applications/LaterKey.app`，开关打开。

之后用同一证书重新打包安装，授权保持有效，不用再来一次。

## 运行形态

- **裸启动**（双击 .app / LaunchAgent）：菜单栏托盘驻留，重映射在后台线程。
- **带子命令**：`laterkey run / doctor / lock / install / uninstall`，走 CLI。
- LaunchAgent：`~/Library/LaunchAgents/io.github.crazykun.laterkey.plist`，指向 `/Applications/LaterKey.app/Contents/MacOS/laterkey run --forever`，`KeepAlive.SuccessfulExit=false`（异常退出自动拉起）。
- 日志：`~/Library/Logs/laterkey/{stdout,stderr}.log`。

## 卸载常驻

```bash
launchctl bootout "gui/$(id -u)" io.github.crazykun.laterkey
rm ~/Library/LaunchAgents/io.github.crazykun.laterkey.plist
rm -rf /Applications/LaterKey.app
```
