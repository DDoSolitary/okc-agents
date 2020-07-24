if (-not (Test-Path Env:ANDROID_SDK_ROOT)) {
	echo "Android SDK not found!"
	exit
}

if (Test-Path Env:TARGET) {
	$target = $Env:TARGET
} else {
	$target = "aarch64-linux-android"
}

$target_env = $target.ToUpper() -replace "-", "_"
$ndk_target = $target -replace "armv7-", "armv7a-"

Set-Content Env:CARGO_TARGET_${target_env}_LINKER "$Env:ANDROID_SDK_ROOT/ndk-bundle/toolchains/llvm/prebuilt/windows-x86_64/bin/${ndk_target}30-clang.cmd"
$Env:RUSTFLAGS = "-C link-arg=-Wl,-rpath=/data/data/com.termux/files/usr/lib -C link-arg=-Wl,--enable-new-dtags"

cargo build --target=$target $args
