#!/bin/bash
# ============================================================
# JCode — .rpm 包构建脚本 (CentOS/RHEL/KylinOS)
#
# 用法: bash deploy/packaging/build_rpm.sh [version]
#       默认版本从 Cargo.toml 读取
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="${1:-$(grep '^version = ' "$PROJECT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')}"
RPM_BUILD_DIR="$PROJECT_DIR/target/rpm"
ARCH="x86_64"

echo "============================================"
echo " 构建 .rpm 包"
echo "  版本: $VERSION"
echo "  架构: $ARCH"
echo "============================================"

# ── 1. 构建二进制 ──
echo ""
echo "[1/5] 编译 release 二进制..."
cd "$PROJECT_DIR"
cargo build --release --bin jcode --bin jcode-server --bin jcode-grpc

# ── 2. 创建 RPM 构建目录 ──
echo ""
echo "[2/5] 创建 RPM 构建目录..."
rm -rf "$RPM_BUILD_DIR"
mkdir -p "$RPM_BUILD_DIR/BUILD"
mkdir -p "$RPM_BUILD_DIR/RPMS/$ARCH"
mkdir -p "$RPM_BUILD_DIR/SOURCES"
mkdir -p "$RPM_BUILD_DIR/SPECS"
mkdir -p "$RPM_BUILD_DIR/SRPMS"

# ── 3. 创建源码 tar ──
echo ""
echo "[3/5] 打包源码..."
TAR_NAME="jcode-${VERSION}.tar.gz"
cd "$RPM_BUILD_DIR"
mkdir -p "jcode-${VERSION}"
cp "$PROJECT_DIR/target/release/jcode" "jcode-${VERSION}/"
cp "$PROJECT_DIR/target/release/jcode-server" "jcode-${VERSION}/"
cp "$PROJECT_DIR/target/release/jcode-grpc" "jcode-${VERSION}/"
cp -r "$PROJECT_DIR/deploy/systemd/jcode-server.service" "jcode-${VERSION}/"
cp "$PROJECT_DIR/packaging/linux/jcode-desktop.desktop" "jcode-${VERSION}/"
cp "$PROJECT_DIR/deploy/selinux/jcode.te" "jcode-${VERSION}/"
tar -czf "SOURCES/$TAR_NAME" "jcode-${VERSION}"
rm -rf "jcode-${VERSION}"

# ── 4. 编写 SPEC ──
echo ""
echo "[4/5] 编写 SPEC..."
cat > "SPECS/jcode.spec" << EOF
Name:       jcode
Version:    ${VERSION}
Release:    1%{?dist}
Summary:    AI-powered development agent

Group:      Development/Tools
License:    MIT
URL:        https://github.com/1jehuang/jcode
Source0:    %{name}-%{version}.tar.gz
BuildArch:  ${ARCH}

Requires:   glibc >= 2.28, openssl >= 1.1

%description
JCode is a blazing-fast coding agent with TUI, multi-model support,
swarm coordination, and 30+ tools. This package provides the
jcode CLI, jcode-server (multi-protocol), and jcode-grpc binaries.

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_unitdir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/selinux/packages
mkdir -p %{buildroot}/var/lib/jcode
mkdir -p %{buildroot}/var/log/jcode

cp %{_builddir}/%{name}-%{version}/jcode %{buildroot}%{_bindir}/
cp %{_builddir}/%{name}-%{version}/jcode-server %{buildroot}%{_bindir}/
cp %{_builddir}/%{name}-%{version}/jcode-grpc %{buildroot}%{_bindir}/
cp %{_builddir}/%{name}-%{version}/jcode-server.service %{buildroot}%{_unitdir}/
cp %{_builddir}/%{name}-%{version}/jcode-desktop.desktop %{buildroot}%{_datadir}/applications/
cp %{_builddir}/%{name}-%{version}/jcode.te %{buildroot}%{_datadir}/selinux/packages/

%post
# 创建 jcode 用户
id -u jcode &>/dev/null || useradd --system --no-create-home --shell /sbin/nologin jcode
chown -R jcode:jcode /var/lib/jcode /var/log/jcode
%systemd_post jcode-server.service

%preun
%systemd_preun jcode-server.service

%postun
%systemd_postun jcode-server.service

%files
%{_bindir}/jcode
%{_bindir}/jcode-server
%{_bindir}/jcode-grpc
%{_unitdir}/jcode-server.service
%{_datadir}/applications/jcode-desktop.desktop
%{_datadir}/selinux/packages/jcode.te
%dir /var/lib/jcode
%dir /var/log/jcode

%changelog
* $(date "+%a %b %d %Y")  JCode Contributors <jcode@example.com> - ${VERSION}-1
- Initial RPM package for JCode ${VERSION}
EOF

# ── 5. 构建 .rpm ──
echo ""
echo "[5/5] 构建 .rpm 包..."
rpmbuild --define "_topdir $RPM_BUILD_DIR" -bb "SPECS/jcode.spec" 2>&1 | \
    tee /tmp/jcode-rpm-build.log

RPM_FILE=$(find "$RPM_BUILD_DIR/RPMS" -name "*.rpm" -type f | head -1)
if [[ -n "$RPM_FILE" ]]; then
    echo ""
    echo "✅ .rpm 包构建成功:"
    echo "   $RPM_FILE"
    echo "  大小: $(du -h "$RPM_FILE" | cut -f1)"
    echo ""
    echo "安装命令:"
    echo "  sudo rpm -ivh $RPM_FILE"
else
    echo "❌ 构建失败，检查日志: /tmp/jcode-rpm-build.log"
    exit 1
fi
