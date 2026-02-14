//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO-GPU 命令常量
//!
//! 参考: https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html#x1-32600010

/// 2D 命令类型
pub mod cmd {
    // 控制队列命令
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;

    // 光标队列命令
    pub const UPDATE_CURSOR: u32 = 0x0200;
    pub const MOVE_CURSOR: u32 = 0x0201;

    // 响应类型
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_OK_CAPSET_INFO: u32 = 0x1102;
    pub const RESP_OK_CAPSET: u32 = 0x1103;
    pub const RESP_ERR_UNSPEC: u32 = 0x1200;
    pub const RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
    pub const RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
    pub const RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
    pub const RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
    pub const RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;
}

/// 格式常量
pub mod format {
    pub const B8G8R8A8_UNORM: u32 = 1;
    pub const B8G8R8X8_UNORM: u32 = 2;
    pub const R8G8B8A8_UNORM: u32 = 3;
    pub const R8G8B8X8_UNORM: u32 = 4;
    pub const X8R8G8B8_UNORM: u32 = 67;
    pub const X8B8G8R8_UNORM: u32 = 68;
    pub const A8R8G8B8_UNORM: u32 = 69;
    pub const A8B8G8R8_UNORM: u32 = 71;
}
