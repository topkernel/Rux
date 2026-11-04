# Init and cross compile

It is possible to cross compile LTX and run it as init. There is a
script to do this and create an initrd suitable for direct booting in
QEMU.

We can also cross compile the kernel and some test executables. The
only dependencies are Zig (`>= 0.11.0`), Clang, `cpio`, `gzip` and
some standard utils like `sh`, `find` etc.

We refer to the directory containing this README as `$ltx`.

## Create the initrd tree

For example, create a directory structure as follows

```sh
$ cd $ltx/cross
$ mkdir initrds
$ mkdir initrds/arm64/bin
```

The `arm64` folder will be the root dir of your image. You can select
a different location if you wish. The only requirement is the `bin`
folder which must contain one or more test executables (added below).

## Build the initrd

First we need to cross compile LTX and a test executable.

```sh
$ cd $ltx
$ zig build -Dtarget=aarch64-linux-musl
$ cp zig-out/bin/ltx cross/initrds/arm64/init
$ cp zig-out/bin/sysinfo cross-initrds/arm64/bin/sysinfo
```

The LTX executable must be renamed to `init` and be placed in the root
of our initrd image. The `sysinfo` program just prints some
information and can be used as a smoke test.

Now we can build the image:

```sh
$ cross/mk-initrd.sh cross/initrds/arm64
```

This will output the image `cross/initrds/arm64.cpio.gz`. Any files
you add to `cross/initrds/arm64` will be available in your root file
system.

## Making a Linux image

The Linux kernel can be cross compiled with LLVM in the following way.

```sh
$ cd $linux_git_checkout
$ make LLVM=1 ARCH=arm64 defconfig
$ make LLVM=1 ARCH=arm64 menuconfig # Optional
$ make LLVM=1 ARCH=arm64 -j$(nproc)
$ mkdir $ltx/cross/kernels
$ cp arch/arm64/boot/Image.gz $ltx/cross/kernels/arm64-Image.gz
```

We need the `Image.gz` file on ARM64, other architectures use
different names. For example on x86 it is `bzImage`.

