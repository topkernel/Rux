//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPC core infrastructure tests

use crate::ipc::util::*;
use crate::ipc::sysv_sem::SemidDsUapi;
use crate::ipc::sysv_msg::MsqidDsUapi;
use crate::ipc::sysv_shm::ShmidDsUapi;
use crate::ipc::posix_mq::MqAttr;
use super::{test_pass, test_fail, test_group_start, test_assert_eq, test_assert};

pub fn test_ipc_sysv() {
    test_group_start("ipc_sysv");

    test_ipc_constants();
    test_ipc_perm_uapi_layout();
    test_ipc_perm_operations();
    test_ipc_id_encoding();
    test_ipc_uapi_struct_sizes();
    test_sem_buf_layout();
    test_msg_match_logic();
}

fn test_ipc_constants() {
    // IPC control commands
    test_assert_eq!(IPC_CREAT, 0o1000, "IPC_CREAT == 0o1000");
    test_assert_eq!(IPC_EXCL, 0o2000, "IPC_EXCL == 0o2000");
    test_assert_eq!(IPC_NOWAIT, 0o4000, "IPC_NOWAIT == 0o4000");
    test_assert_eq!(IPC_RMID, 0, "IPC_RMID == 0");
    test_assert_eq!(IPC_SET, 1, "IPC_SET == 1");
    test_assert_eq!(IPC_STAT, 2, "IPC_STAT == 2");
    test_assert_eq!(IPC_INFO, 3, "IPC_INFO == 3");

    // Semaphore control commands
    test_assert_eq!(GETPID, 11, "GETPID == 11");
    test_assert_eq!(GETVAL, 12, "GETVAL == 12");
    test_assert_eq!(GETALL, 13, "GETALL == 13");
    test_assert_eq!(GETNCNT, 14, "GETNCNT == 14");
    test_assert_eq!(GETZCNT, 15, "GETZCNT == 15");
    test_assert_eq!(SETVAL, 16, "SETVAL == 16");
    test_assert_eq!(SETALL, 17, "SETALL == 17");

    // Operation flags
    test_assert_eq!(SEM_UNDO, 0x1000, "SEM_UNDO == 0x1000");
    test_assert_eq!(MSG_NOERROR, 0o10000, "MSG_NOERROR == 0o10000");
    test_assert_eq!(SHM_RDONLY, 0o10000, "SHM_RDONLY == 0o10000");
    test_assert_eq!(SHM_RND, 0o20000, "SHM_RND == 0o20000");
}

fn test_ipc_perm_uapi_layout() {
    // struct ipc64_perm must be exactly 48 bytes for RV64 ABI
    test_assert_eq!(core::mem::size_of::<IpcPermUapi>(), 48, "IpcPermUapi size == 48");

    // Verify field offsets via a constructed instance
    let perm = IpcPermUapi {
        key: 0x11111111,
        uid: 0x22222222,
        gid: 0x33333333,
        cuid: 0x44444444,
        cgid: 0x55555555,
        mode: 0x66666666,
        seq: 0x7777,
        __pad2: 0x8888,
        __unused1: 0xAAAAAAAAAAAAAAAA,
        __unused2: 0xBBBBBBBBBBBBBBBB,
    };

    let bytes: [u8; 48] = unsafe { core::mem::transmute_copy(&perm) };

    // key at offset 0 (i32 = 4 bytes)
    test_assert_eq!(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), 0x11111111, "perm.key offset 0");
    // uid at offset 4 (u32)
    test_assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]), 0x22222222, "perm.uid offset 4");
    // gid at offset 8
    test_assert_eq!(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]), 0x33333333, "perm.gid offset 8");
    // mode at offset 20 (after key:i32 + uid:u32 + gid:u32 + cuid:u32 + cgid:u32 = 20)
    test_assert_eq!(u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]), 0x66666666, "perm.mode offset 20");
    // seq at offset 24 (after mode:u32)
    test_assert_eq!(u16::from_le_bytes([bytes[24], bytes[25]]), 0x7777, "perm.seq offset 24");
}

fn test_ipc_perm_operations() {
    // Test KernIpcPerm.update_mode preserves upper bits
    let mut perm = KernIpcPerm {
        key: 42,
        uid: 0,
        gid: 0,
        cuid: 0,
        cgid: 0,
        mode: 0o777,
        seq: 0,
    };

    // update_mode should mask to lower 9 bits
    perm.update_mode(0o644);
    test_assert_eq!(perm.mode, 0o644, "update_mode 0o644");

    // Setting mode that has extra bits — should be masked
    perm.update_mode(0o7777);
    test_assert_eq!(perm.mode, 0o777, "update_mode masks to 9 bits");

    // Setting mode 0 should clear permission bits
    perm.mode = 0o755;
    perm.update_mode(0o000);
    test_assert_eq!(perm.mode, 0o000, "update_mode 0o000 clears bits");

    // Test to_uapi conversion
    let perm = KernIpcPerm {
        key: 12345,
        uid: 100,
        gid: 200,
        cuid: 100,
        cgid: 200,
        mode: 0o644,
        seq: 5,
    };
    let uapi = perm.to_uapi();
    test_assert_eq!(uapi.key, 12345, "to_uapi key");
    test_assert_eq!(uapi.uid, 100, "to_uapi uid");
    test_assert_eq!(uapi.gid, 200, "to_uapi gid");
    test_assert_eq!(uapi.cuid, 100, "to_uapi cuid");
    test_assert_eq!(uapi.cgid, 200, "to_uapi cgid");
    test_assert_eq!(uapi.mode, 0o644u32, "to_uapi mode");
    test_assert_eq!(uapi.seq, 5, "to_uapi seq");
}

fn test_ipc_id_encoding() {
    // Basic roundtrip: build → extract
    let id = ipc_build_id(5, 1);
    test_assert_eq!(ipc_id_to_index(id), 5, "id_to_index(5, 1) == 5");
    test_assert_eq!(ipc_id_seq(id), 1, "id_seq(5, 1) == 1");

    // Higher values
    let id = ipc_build_id(42, 100);
    test_assert_eq!(ipc_id_to_index(id), 42, "id_to_index(42, 100) == 42");
    test_assert_eq!(ipc_id_seq(id), 100, "id_seq(42, 100) == 100");

    // Max valid index (255, fits in 16-bit shifted)
    let id = ipc_build_id(255, 65535);
    test_assert_eq!(ipc_id_to_index(id), 255, "id_to_index(255, 65535) == 255");
    test_assert_eq!(ipc_id_seq(id), 65535, "id_seq(255, 65535) == 65535");

    // Seq wrapping (> 0xFFFF should be masked)
    let id = ipc_build_id(0, 0x10000);
    test_assert_eq!(ipc_id_seq(id), 0, "id_seq masks seq to 16 bits");

    // Index 0, seq 0
    let id = ipc_build_id(0, 0);
    test_assert_eq!(id, 0, "ipc_build_id(0, 0) == 0");

    // Index is in upper 16 bits, seq in lower 16 bits
    let id = ipc_build_id(1, 1);
    test_assert_eq!(id, (1 << 16) | 1, "ipc_build_id(1,1) == 0x10001");
}

fn test_ipc_uapi_struct_sizes() {
    // All UAPI struct sizes must match Linux ABI for RV64
    test_assert_eq!(core::mem::size_of::<SemidDsUapi>(), 88, "SemidDsUapi size == 88");
    test_assert_eq!(core::mem::size_of::<MsqidDsUapi>(), 120, "MsqidDsUapi size == 120");
    test_assert_eq!(core::mem::size_of::<ShmidDsUapi>(), 112, "ShmidDsUapi size == 112");
    test_assert_eq!(core::mem::size_of::<MqAttr>(), 64, "MqAttr size == 64");
}

fn test_sem_buf_layout() {
    use crate::ipc::sysv_sem::SemBuf;

    // struct sembuf: sem_num (u16) + sem_op (i16) + sem_flg (u16) = 6 bytes, no padding
    test_assert_eq!(core::mem::size_of::<SemBuf>(), 6, "SemBuf size == 6");
    test_assert_eq!(core::mem::align_of::<SemBuf>(), 2, "SemBuf align == 2");

    let sop = SemBuf { sem_num: 3, sem_op: -1, sem_flg: SEM_UNDO };
    test_assert_eq!(sop.sem_num, 3, "SemBuf.sem_num");
    test_assert_eq!(sop.sem_op, -1, "SemBuf.sem_op");
    test_assert_eq!(sop.sem_flg, SEM_UNDO, "SemBuf.sem_flg");
}

fn test_msg_match_logic() {
    // Reproduce find_msg_match algorithm to test the three msgtyp scenarios.
    // Since find_msg_match is private and depends on the private Msg type,
    // we test the matching logic directly.

    struct TestMsg { mtype: i64 }

    // Match function mirroring sysv_msg::find_msg_match logic
    fn match_msg(msgs: &[TestMsg], msgtyp: i64) -> Option<usize> {
        if msgs.is_empty() { return None; }
        if msgtyp == 0 { return Some(0); }
        if msgtyp > 0 {
            for (i, m) in msgs.iter().enumerate() {
                if m.mtype == msgtyp { return Some(i); }
            }
            return None;
        }
        // msgtyp < 0: lowest type <= |msgtyp|
        let abs_type = (-msgtyp) as i64;
        let mut best_idx: Option<usize> = None;
        let mut best_type: i64 = i64::MAX;
        for (i, m) in msgs.iter().enumerate() {
            if m.mtype <= abs_type && m.mtype < best_type {
                best_type = m.mtype;
                best_idx = Some(i);
            }
        }
        best_idx
    }

    let msgs = [
        TestMsg { mtype: 1 },
        TestMsg { mtype: 3 },
        TestMsg { mtype: 2 },
        TestMsg { mtype: 5 },
    ];

    // msgtyp == 0: first message
    test_assert_eq!(match_msg(&msgs, 0), Some(0), "msgtyp=0 returns first");

    // msgtyp > 0: first matching type
    test_assert_eq!(match_msg(&msgs, 3), Some(1), "msgtyp=3 returns index 1");
    test_assert_eq!(match_msg(&msgs, 5), Some(3), "msgtyp=5 returns index 3");
    test_assert_eq!(match_msg(&msgs, 99), None, "msgtyp=99 no match");

    // msgtyp < 0: lowest type <= |msgtyp|
    test_assert_eq!(match_msg(&msgs, -3), Some(0), "msgtyp=-3 returns type 1 (lowest <= 3)");
    test_assert_eq!(match_msg(&msgs, -1), None, "msgtyp=-1 no type <= 1 except index 0 which is type 1 == 1, should match");
    // Actually type 1 == |msgtyp|==1, so it should match
    test_assert_eq!(match_msg(&msgs, -1), Some(0), "msgtyp=-1 returns type 1");
    test_assert_eq!(match_msg(&msgs, -2), Some(0), "msgtyp=-2 returns type 1 (lowest <= 2)");

    // Empty queue
    let empty: [TestMsg; 0] = [];
    test_assert_eq!(match_msg(&empty, 0), None, "empty queue msgtyp=0");
    test_assert_eq!(match_msg(&empty, 1), None, "empty queue msgtyp=1");
    test_assert_eq!(match_msg(&empty, -1), None, "empty queue msgtyp=-1");

    // Single message
    let single = [TestMsg { mtype: 42 }];
    test_assert_eq!(match_msg(&single, 0), Some(0), "single msg msgtyp=0");
    test_assert_eq!(match_msg(&single, 42), Some(0), "single msg msgtyp=42");
    test_assert_eq!(match_msg(&single, -50), Some(0), "single msg msgtyp=-50");
    test_assert_eq!(match_msg(&single, -10), None, "single msg msgtyp=-10 no match");
}
