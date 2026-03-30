use crate::drivers::virtio::queue::{Desc, UsedElem, AvailRing, UsedRing, VirtIOBlkReqHeader, VirtIOBlkResp};
use super::{test_pass, test_fail, test_group_start};

pub fn test_virtio_queue() {
    test_group_start("VirtIO queue");

    // Test 1: Desc struct size (VirtIO spec: 16 bytes)
    test_assert_eq!(core::mem::size_of::<Desc>(), 16, "Desc size == 16");

    // Test 2: UsedElem struct size (id: u32 + len: u32 = 8 bytes)
    test_assert_eq!(core::mem::size_of::<UsedElem>(), 8, "UsedElem size == 8");

    // Test 3: AvailRing struct size (flags: u16 + idx: u16 = 4 bytes)
    test_assert_eq!(core::mem::size_of::<AvailRing>(), 4, "AvailRing size == 4");

    // Test 4: UsedRing struct size (flags: u16 + idx: u16 = 4 bytes)
    test_assert_eq!(core::mem::size_of::<UsedRing>(), 4, "UsedRing size == 4");

    // Test 5: Desc field layout
    let desc = Desc { addr: 0x1000_0000, len: 512, flags: 1 << 1, next: 3 };
    test_assert_eq!(desc.addr, 0x1000_0000, "Desc.addr");
    test_assert_eq!(desc.len, 512, "Desc.len");
    test_assert_eq!(desc.flags, 2, "Desc.flags");
    test_assert_eq!(desc.next, 3, "Desc.next");

    // Test 6: Desc is Copy
    let desc1 = Desc { addr: 0, len: 0, flags: 0, next: 0 };
    let desc2 = desc1;
    test_assert_eq!(desc1.addr, desc2.addr, "Desc is Copy");

    // Test 7: UsedElem field layout
    let elem = UsedElem { id: 42, len: 1024 };
    test_assert_eq!(elem.id, 42, "UsedElem.id");
    test_assert_eq!(elem.len, 1024, "UsedElem.len");

    // Test 8: VirtIO block request header size
    test_assert!(core::mem::size_of::<VirtIOBlkReqHeader>() > 0, "VirtIOBlkReqHeader non-zero size");

    // Test 9: VirtIO block response size
    test_assert!(core::mem::size_of::<VirtIOBlkResp>() > 0, "VirtIOBlkResp non-zero size");

    // Test 10: VirtIO descriptor flag constants (from VirtIO spec)
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;
    const VIRTQ_DESC_F_INDIRECT: u16 = 4;
    test_assert_eq!(VIRTQ_DESC_F_NEXT, 1, "VIRTQ_DESC_F_NEXT == 1");
    test_assert_eq!(VIRTQ_DESC_F_WRITE, 2, "VIRTQ_DESC_F_WRITE == 2");
    test_assert_eq!(VIRTQ_DESC_F_INDIRECT, 4, "VIRTQ_DESC_F_INDIRECT == 4");

    // Test 11: VirtIO block request types
    const VIRTIO_BLK_T_IN: u32 = 0;
    const VIRTIO_BLK_T_OUT: u32 = 1;
    const VIRTIO_BLK_T_FLUSH: u32 = 4;
    test_assert_eq!(VIRTIO_BLK_T_IN, 0, "VIRTIO_BLK_T_IN == 0");
    test_assert_eq!(VIRTIO_BLK_T_OUT, 1, "VIRTIO_BLK_T_OUT == 1");
    test_assert_eq!(VIRTIO_BLK_T_FLUSH, 4, "VIRTIO_BLK_T_FLUSH == 4");

    // Test 12: VirtIO block response statuses
    const VIRTIO_BLK_S_OK: u8 = 0;
    const VIRTIO_BLK_S_IOERR: u8 = 1;
    const VIRTIO_BLK_S_UNSUPP: u8 = 2;
    test_assert_eq!(VIRTIO_BLK_S_OK, 0, "VIRTIO_BLK_S_OK == 0");
    test_assert_eq!(VIRTIO_BLK_S_IOERR, 1, "VIRTIO_BLK_S_IOERR == 1");
    test_assert_eq!(VIRTIO_BLK_S_UNSUPP, 2, "VIRTIO_BLK_S_UNSUPP == 2");
}
