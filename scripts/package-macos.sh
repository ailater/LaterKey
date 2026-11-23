#!/usr/bin/env bash
# LaterKey macOS 打包：release 二进制 → 签名的 .app（+ 可选 dmg）。
#
# 用法：
#   scripts/package-macos.sh                # 本机架构，出 .app
#   scripts/package-macos.sh --universal    # arm64+x86_64 胖包，出 .app 与 dmg
#   scripts/package-macos.sh --install      # 打包后装到 /Applications 并注册 LaunchAgent
#
# 签名（沿用 LaterScreen 的思路）：LATERKEY_SIGN_IDENTITY 指定证书名时用该证书
# 签——TCC 绑定证书身份，**跨版本升级不用重新授权辅助功能**。缺省 ad-hoc（cdhash
# 锚定，同版本内稳定，每次升级要重新授权一次）。证书生成见 docs/SIGNING.md。
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${LATERKEY_VERSION:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)}
DIST=dist
APP_NAME=LaterKey
BUNDLE="$DIST/$APP_NAME.app"
UNIVERSAL=0
DO_INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --universal) UNIVERSAL=1 ;;
        --install) DO_INSTALL=1 ;;
        *) echo "未知参数：$arg" >&2; exit 1 ;;
    esac
done

build() {
    if [[ $UNIVERSAL == 1 ]]; then
        for t in aarch64-apple-darwin x86_64-apple-darwin; do
            cargo build --release --target "$t"
        done
        mkdir -p target/universal
        lipo -create \
            target/aarch64-apple-darwin/release/laterkey \
            target/x86_64-apple-darwin/release/laterkey \
            -output target/universal/laterkey
        lipo -info target/universal/laterkey
        echo "target/universal/laterkey"
    else
        cargo build --release
        echo "target/release/laterkey"
    fi
}

assemble() { # $1=二进制路径
    rm -rf "$BUNDLE"
    mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
    install -m755 "$1" "$BUNDLE/Contents/MacOS/laterkey"
    sed "s/__VERSION__/$VERSION/g" packaging/macos/Info.plist.in > "$BUNDLE/Contents/Info.plist"

    # png -> icns（系统自带 sips/iconutil）
    if [[ -f packaging/icon.png ]]; then
        local iconset="$DIST/.stage/icon.iconset"
        rm -rf "$iconset" && mkdir -p "$iconset"
        local s
        for s in 16 32 64 128 256; do
            sips -z "$s" "$s" packaging/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
            sips -z "$((s * 2))" "$((s * 2))" packaging/icon.png \
                --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
        done
        iconutil -c icns "$iconset" -o "$BUNDLE/Contents/Resources/icon.icns"
    fi

    # 先签内部二进制、再签 bundle（密封 Info.plist 与资源）。未密封的 bundle
    # 进程身份校验会失败——授权后仍反复弹窗（LaterScreen 真机踩过）。
    local sign_args=()
    if [[ -n ${LATERKEY_SIGN_IDENTITY:-} ]]; then
        sign_args=(--force --timestamp=none -s "$LATERKEY_SIGN_IDENTITY")
        [[ -n ${LATERKEY_SIGN_KEYCHAIN:-} ]] && sign_args+=(--keychain "$LATERKEY_SIGN_KEYCHAIN")
    else
        sign_args=(--force -s -)
    fi
    codesign "${sign_args[@]}" "$BUNDLE/Contents/MacOS/laterkey"
    codesign "${sign_args[@]}" "$BUNDLE"
    codesign --verify --verbose "$BUNDLE"
    echo "已打包：$BUNDLE"
}

make_dmg() {
    local stage="$DIST/.stage/dmg"
    rm -rf "$stage" && mkdir -p "$stage"
    cp -R "$BUNDLE" "$stage/"
    ln -s /Applications "$stage/Applications"
    local out="$DIST/laterkey-v$VERSION-universal2-apple-darwin.dmg"
    rm -f "$out"
    hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$stage" -format UDZO "$out"
    echo "    $out ($(du -h "$out" | cut -f1))"
}

install_agent() {
    # 装到 /Applications（TCC 与 LaunchAgent 都按这个固定路径认）
    if [[ -d /Applications/$APP_NAME.app ]]; then
        launchctl bootout "gui/$(id -u)" io.github.crazykun.laterkey 2>/dev/null || true
        rm -rf "/Applications/$APP_NAME.app"
    fi
    cp -R "$BUNDLE" "/Applications/$APP_NAME.app"
    mkdir -p ~/Library/LaunchAgents ~/Library/Logs/laterkey
    cat > ~/Library/LaunchAgents/io.github.crazykun.laterkey.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>io.github.crazykun.laterkey</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/$APP_NAME.app/Contents/MacOS/laterkey</string>
        <string>run</string>
        <string>--forever</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>$HOME/Library/Logs/laterkey/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>$HOME/Library/Logs/laterkey/stderr.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
EOF
    launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/io.github.crazykun.laterkey.plist 2>/dev/null || \
        launchctl bootout "gui/$(id -u)" io.github.crazykun.laterkey 2>/dev/null && \
        launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/io.github.crazykun.laterkey.plist
    echo "已安装 /Applications/$APP_NAME.app 并注册 LaunchAgent"
}

BIN=$(build | tail -1)
assemble "$BIN"
# 注意：不能用 `[[ ... ]] && cmd` 收尾——条件不成立时整条命令返回 1，
# 脚本退出码随之变 1，CI 会把成功的打包判成失败（v0.1.0 首次发布踩过）。
if [[ $UNIVERSAL == 1 ]]; then
    make_dmg
fi
if [[ $DO_INSTALL == 1 ]]; then
    install_agent
fi
exit 0
