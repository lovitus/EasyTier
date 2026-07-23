#!/bin/sh

version=$(cat module.prop | grep 'version=' | awk -F '=' '{print $2}' | sed 's/ (.*//')

version='v'$(grep '^version =' ../../easytier/Cargo.toml | cut -d '"' -f 2)

if [ -z "$version" ]; then
    echo "Error: 版本号不存在."
    exit 1
fi

filename="easytier_magisk_${version}.zip"
echo $version  


has_release_payload() {
    for binary in \
        easytier-core \
        easytier-cli \
        easytier-web \
        easytier-leaf-worker \
        easytier-hev-socks-egress
    do
        [ -f "./$binary" ] || return 1
    done
}

if has_release_payload; then
    zip -r -o -X "$filename" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'
else
    wget -O "easytier_last.zip" https://github.com/EasyTier/EasyTier/releases/download/"$version"/easytier-linux-aarch64-"$version".zip
    unzip -o easytier_last.zip -d ./
    mv ./easytier-linux-aarch64/* ./
    rm -rf ./easytier_last.zip
    rm -rf ./easytier-linux-aarch64
    if ! has_release_payload; then
        echo "Error: release payload is missing EasyTier, Leaf, or HEV binaries."
        exit 1
    fi
    zip -r -o -X "$filename" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'
fi
