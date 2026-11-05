#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
export SOURCE_DATE_EPOCH
export TZ=UTC

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/.." && pwd)"
invocation_dir="$(pwd)"
package_name="buisc"
maintainer="${BUISC_MAINTAINER:-Buisc contributors <buisc@users.noreply.github.com>}"

usage() {
    cat <<'EOF'
用法：scripts/package.sh [选项]

为已经构建好的 release 二进制创建 Debian 或 RPM 软件包。

选项：
  --format deb|rpm|all  打包格式，默认 all
  --arch amd64|arm64    Debian 架构名，默认当前主机架构
  --binary PATH         release 二进制路径，默认 target/release/bili-music
  --output PATH         输出目录，默认 dist
  -h, --help            显示此帮助

环境变量：
  SOURCE_DATE_EPOCH     可重复构建时间；默认取当前 Git 提交时间
  BUISC_MAINTAINER      Debian Maintainer 字段
EOF
}

fail() {
    printf '打包失败：%s\n' "$*" >&2
    exit 1
}

absolute_from_invocation() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$invocation_dir" "$1" ;;
    esac
}

detect_default_arch() {
    case "$(uname -m)" in
        x86_64|amd64) printf 'amd64\n' ;;
        aarch64|arm64) printf 'arm64\n' ;;
        *) fail "不支持的主机架构：$(uname -m)" ;;
    esac
}

package_version() {
    awk '
        /^\[package\][[:space:]]*$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^version[[:space:]]*=/ {
            value = $0
            sub(/^[^=]*=[[:space:]]*"/, "", value)
            sub(/".*$/, "", value)
            print value
            exit
        }
    ' "$project_root/Cargo.toml"
}

validate_elf_arch() {
    command -v file >/dev/null 2>&1 || return 0

    local description
    description="$(file -b -- "$binary")"
    [[ "$description" == *ELF* ]] || fail "不是 ELF 文件：$binary"

    case "$arch" in
        amd64)
            [[ "$description" == *"x86-64"* ]] \
                || fail "二进制架构与 --arch amd64 不匹配：$description"
            ;;
        arm64)
            [[ "$description" == *aarch64* || "$description" == *"ARM aarch64"* ]] \
                || fail "二进制架构与 --arch arm64 不匹配：$description"
            ;;
    esac
}

normalise_mtimes() {
    find "$1" -depth -exec touch -h -d "@$SOURCE_DATE_EPOCH" -- {} +
}

render_template() {
    local template=$1
    local destination=$2
    local escaped_maintainer escaped_version
    escaped_maintainer="$(printf '%s' "$maintainer" | sed 's/[&|\]/\\&/g')"
    escaped_version="$(printf '%s' "$version" | sed 's/[&|\]/\\&/g')"
    sed \
        -e "s|@MAINTAINER@|$escaped_maintainer|g" \
        -e "s|@VERSION@|$escaped_version|g" \
        "$template" > "$destination"
}

package_deb() {
    local package_root="$work_root/deb"
    local installed_bytes installed_kib control_tmp

    install -d \
        "$package_root/DEBIAN" \
        "$package_root/usr/bin" \
        "$package_root/usr/share/applications" \
        "$package_root/usr/share/doc/$package_name" \
        "$package_root/usr/share/icons/hicolor/scalable/apps"

    install -m0755 -- "$binary" "$package_root/usr/bin/buisc"
    install -m0644 -- "$project_root/packaging/desktop/io.github.bilimusic.Linux.desktop" \
        "$package_root/usr/share/applications/io.github.bilimusic.Linux.desktop"
    install -m0644 -- "$project_root/assets/io.github.bilimusic.Linux.svg" \
        "$package_root/usr/share/icons/hicolor/scalable/apps/io.github.bilimusic.Linux.svg"
    install -m0644 -- "$project_root/LICENSE" \
        "$package_root/usr/share/doc/$package_name/copyright"

    installed_bytes="$(
        find "$package_root/usr" -type f -printf '%s\n' \
            | awk '{ total += $1 } END { printf "%.0f\n", total }'
    )"
    installed_kib=$(((installed_bytes + 1023) / 1024))

    control_tmp="$work_root/deb-control"
    render_template "$project_root/packaging/deb/control.in" "$control_tmp"
    {
        sed \
            -e "s|@ARCH@|$arch|g" \
            -e "s|@INSTALLED_SIZE@|$installed_kib|g" \
            "$control_tmp"
    } > "$package_root/DEBIAN/control"
    chmod 0644 "$package_root/DEBIAN/control"

    (
        cd "$package_root"
        find usr -type f -printf '%p\0' \
            | sort -z \
            | xargs -0 md5sum > DEBIAN/md5sums
    )
    chmod 0644 "$package_root/DEBIAN/md5sums"

    normalise_mtimes "$package_root"
    deb_artifact="$work_root/${package_name}_${version}_${arch}.deb"
    dpkg-deb --build --root-owner-group -Zxz "$package_root" "$deb_artifact" >/dev/null
}

package_rpm() {
    local rpm_top="$work_root/rpmbuild"
    local rendered_spec="$work_root/${package_name}.spec"
    local rpm_arch

    case "$arch" in
        amd64) rpm_arch=x86_64 ;;
        arm64) rpm_arch=aarch64 ;;
    esac

    install -d \
        "$rpm_top/BUILD" \
        "$rpm_top/BUILDROOT" \
        "$rpm_top/RPMS" \
        "$rpm_top/SOURCES" \
        "$rpm_top/SPECS" \
        "$rpm_top/SRPMS"
    render_template "$project_root/packaging/rpm/buisc.spec.in" "$rendered_spec"

    if ! rpmbuild \
        --define "_topdir $rpm_top" \
        --define "_buisc_binary $binary" \
        --define "_buisc_source_root $project_root" \
        --define "_buildhost reproducible" \
        --define "use_source_date_epoch_as_buildtime 1" \
        --define "clamp_mtime_to_source_date_epoch 1" \
        --define "_binary_payload w9.gzdio" \
        --target "$rpm_arch" \
        -bb "$rendered_spec" >"$work_root/rpmbuild.log" 2>&1; then
        cat "$work_root/rpmbuild.log" >&2
        fail "rpmbuild 执行失败"
    fi

    rpm_artifact="$(find "$rpm_top/RPMS" -type f \
        -name "${package_name}-${version}-1.${rpm_arch}.rpm" -print -quit)"
    [[ -n "$rpm_artifact" ]] || fail "rpmbuild 未生成预期的 RPM 包"
}

format=all
arch="$(detect_default_arch)"
binary="$project_root/target/release/bili-music"
output="$project_root/dist"

while (($#)); do
    case "$1" in
        --format)
            (($# >= 2)) || fail "--format 缺少参数"
            format=$2
            shift 2
            ;;
        --arch)
            (($# >= 2)) || fail "--arch 缺少参数"
            arch=$2
            shift 2
            ;;
        --binary)
            (($# >= 2)) || fail "--binary 缺少参数"
            binary="$(absolute_from_invocation "$2")"
            shift 2
            ;;
        --output)
            (($# >= 2)) || fail "--output 缺少参数"
            output="$(absolute_from_invocation "$2")"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "未知参数：$1"
            ;;
    esac
done

case "$format" in
    deb|rpm|all) ;;
    *) fail "--format 必须是 deb、rpm 或 all" ;;
esac
case "$arch" in
    amd64|arm64) ;;
    *) fail "--arch 必须是 amd64 或 arm64" ;;
esac

[[ -f "$binary" ]] || fail "找不到 release 二进制：$binary"
[[ -x "$binary" ]] || fail "release 二进制不可执行：$binary"
[[ -r "$project_root/packaging/deb/control.in" ]] || fail "缺少 Debian control 模板"
[[ -r "$project_root/packaging/rpm/buisc.spec.in" ]] || fail "缺少 RPM spec 模板"
[[ -r "$project_root/packaging/desktop/io.github.bilimusic.Linux.desktop" ]] \
    || fail "缺少桌面入口"
[[ -r "$project_root/assets/io.github.bilimusic.Linux.svg" ]] || fail "缺少图标资源"

case "$format" in
    deb) command -v dpkg-deb >/dev/null 2>&1 || fail "需要 dpkg-deb" ;;
    rpm) command -v rpmbuild >/dev/null 2>&1 || fail "需要 rpmbuild" ;;
    all)
        command -v dpkg-deb >/dev/null 2>&1 || fail "打包 all 需要 dpkg-deb"
        command -v rpmbuild >/dev/null 2>&1 || fail "打包 all 需要 rpmbuild"
        ;;
esac

validate_elf_arch

version="$(package_version)"
[[ -n "$version" ]] || fail "无法从 Cargo.toml 读取版本"

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    if command -v git >/dev/null 2>&1 \
        && git -C "$project_root" rev-parse --verify HEAD >/dev/null 2>&1; then
        SOURCE_DATE_EPOCH="$(git -C "$project_root" log -1 --format=%ct)"
    else
        SOURCE_DATE_EPOCH=0
    fi
fi
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] \
    || fail "SOURCE_DATE_EPOCH 必须是非负整数"

work_root="$(mktemp -d "${TMPDIR:-/tmp}/buisc-package.XXXXXXXX")"
trap 'rm -rf -- "$work_root"' EXIT

artifacts=()
case "$format" in
    deb)
        package_deb
        artifacts+=("$deb_artifact")
        ;;
    rpm)
        package_rpm
        artifacts+=("$rpm_artifact")
        ;;
    all)
        package_deb
        package_rpm
        artifacts+=("$deb_artifact" "$rpm_artifact")
        ;;
esac

install -d -m0755 -- "$output"
install -m0644 -- "${artifacts[@]}" "$output/"
printf '已生成 Buisc %s（%s）软件包：\n' "$version" "$arch"
for artifact in "${artifacts[@]}"; do
    sha256sum -- "$output/$(basename -- "$artifact")"
done
