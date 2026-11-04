arch=$1
initrd=$2
kernel=$3

Q=qemu-system

case $arch in
        arm64|aarch64) \
                $Q-aarch64 -m 1G \
                           -smp 2 \
                           -display none \
                           -kernel $kernel \
                           -initrd $initrd \
                           -machine virt -cpu cortex-a57 \
                           -serial stdio \
                           -append 'console=ttyAMA0 earlyprintk=ttyAMA0';;
        *) echo "Don't recognise $arch"
           exit 1;;
esac

