#!/usr/bin/sh -eu

rootdir=$(realpath $1)

if [ -z $rootdir ]; then
        echo "Expected directory as first positional parameter"
        exit 1
fi

if [ ! -d $rootdir/bin ]; then
        echo "Expected bin directory in $rootdir"
        exit 1
else
        tests=$(find $rootdir/bin -type f -executable)
        if [ -z $tests ]; then
                echo "Expected tests in $rootdir/bin"
                exit 1
        fi
fi

if [ ! -x $rootdir/init ]; then
        echo "Expected init executable (LTX) in $rootdir"
        exit 1
fi

bname=$(basename $rootdir)
pdir=$(pwd)
cd $rootdir
find . | cpio -v -H newc -o | gzip -n > ../$bname.cpio.gz
cd $pdir
