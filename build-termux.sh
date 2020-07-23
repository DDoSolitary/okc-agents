#!/bin/bash

if [[ -z $ANDROID_SDK_ROOT ]]; then
	echo Android SDK not found!
	exit
fi

TARGET=${TARGET:-aarch64-linux-android}
TARGET_ENV=${TARGET^^}
TARGET_ENV=${TARGET_ENV//-/_}
NDK_TARGET=${TARGET/armv7-/armv7a-}

export CARGO_TARGET_${TARGET_ENV}_LINKER=$ANDROID_SDK_ROOT/ndk-bundle/toolchains/llvm/prebuilt/linux-x86_64/bin/${NDK_TARGET}30-clang
export RUSTFLAGS="-C link-arg=-Wl,-rpath=/data/data/com.termux/files/usr/lib -C link-arg=-Wl,--enable-new-dtags"

cargo build --target=$TARGET $@
