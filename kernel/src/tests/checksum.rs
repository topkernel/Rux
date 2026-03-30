use crate::net::ipv4::checksum::{ip_checksum, verify_ip_checksum, pseudo_header_checksum};
use super::{test_pass, test_fail, test_group_start};

pub fn test_checksum() {
    test_group_start("checksum");

    // Test 1: ip_checksum produces deterministic result
    let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00,
                0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8, 0x01, 0x01,
                0xc0, 0xa8, 0x01, 0x02];
    let csum1 = ip_checksum(&data);
    let csum2 = ip_checksum(&data);
    // Verify determinism
    if csum1 == csum2 {
        test_pass("ip_checksum deterministic");
    } else {
        test_fail("ip_checksum deterministic", "different results");
    }
    // Verify non-zero result
    if csum1 != 0 {
        test_pass("ip_checksum non-zero for arbitrary data");
    } else {
        test_fail("ip_checksum non-zero", "unexpected zero");
    }

    // Test 2: All-zeros data
    let zeros = [0u8; 8];
    let csum = ip_checksum(&zeros);
    // Sum of 4 zero words = 0, fold = 0, complement = 0xFFFF
    if csum == 0xFFFF {
        test_pass("ip_checksum all zeros = 0xFFFF");
    } else {
        test_fail("ip_checksum all zeros", &alloc::format!("got {:#06x}", csum));
    }

    // Test 3: All-ones data (0xFF bytes)
    let ones = [0xFFu8; 8];
    let csum = ip_checksum(&ones);
    // 4 words of 0xFFFF sum = 0x3FFFC, fold = 0xFFFC + 3 = 0xFFFF, complement = 0
    if csum == 0x0000 {
        test_pass("ip_checksum all ones = 0x0000");
    } else {
        test_fail("ip_checksum all ones", &alloc::format!("got {:#06x}", csum));
    }

    // Test 4: Single byte (odd length)
    // Odd length: 0x45 << 8 = 0x4500, complement = 0xBAFF
    let single = [0x45u8; 1];
    let csum = ip_checksum(&single);
    if csum != 0 && csum != 0xFFFF {
        test_pass("ip_checksum single byte odd length");
    } else {
        test_fail("ip_checksum single byte", &alloc::format!("got {:#06x}", csum));
    }

    // Test 5: Empty data
    let empty: [u8; 0] = [];
    let csum = ip_checksum(&empty);
    // Sum = 0, complement = 0xFFFF
    if csum == 0xFFFF {
        test_pass("ip_checksum empty data = 0xFFFF");
    } else {
        test_fail("ip_checksum empty data", &alloc::format!("got {:#06x}", csum));
    }

    // Test 6: verify_ip_checksum — construct valid packet
    // Build a packet where we compute the correct checksum
    let mut packet = [0x45u8; 20];
    packet[2] = 0x00; // total length high
    packet[3] = 0x14; // total length = 20
    packet[8] = 64;   // TTL
    packet[9] = 6;    // protocol = TCP
    // Zero checksum field for computation
    packet[10] = 0;
    packet[11] = 0;
    // Compute correct checksum
    let correct_csum = ip_checksum(&packet);
    packet[10] = (correct_csum >> 8) as u8;
    packet[11] = (correct_csum & 0xFF) as u8;
    // Now verify should return true
    if verify_ip_checksum(&packet) {
        test_pass("verify_ip_checksum valid packet (computed)");
    } else {
        test_fail("verify_ip_checksum valid", "computed checksum failed verification");
    }

    // Test 7: verify_ip_checksum with corrupted packet
    let mut corrupted = packet;
    corrupted[0] = 0x47; // Change version
    if !verify_ip_checksum(&corrupted) {
        test_pass("verify_ip_checksum corrupted packet");
    } else {
        test_fail("verify_ip_checksum corrupted", "should detect corruption");
    }

    // Test 8: verify_ip_checksum with wrong checksum byte
    let mut bad_csum = packet;
    bad_csum[10] = 0x00; // Zero out checksum
    bad_csum[11] = 0x00;
    if !verify_ip_checksum(&bad_csum) {
        test_pass("verify_ip_checksum zeroed checksum");
    } else {
        test_fail("verify_ip_checksum zeroed", "should detect bad checksum");
    }

    // Test 9: pseudo_header_checksum determinism
    let src = 0xC0A80101;
    let dst = 0xC0A80102;
    let csum1 = pseudo_header_checksum(src, dst, 6, 20);
    let csum2 = pseudo_header_checksum(src, dst, 6, 20);
    test_assert!(csum1 == csum2, "pseudo_header_checksum deterministic");

    // Test 10: pseudo_header_checksum non-zero result
    test_assert!(csum1 != 0, "pseudo_header_checksum non-zero");

    // Test 11: pseudo_header_checksum with different protocols
    let tcp_csum = pseudo_header_checksum(src, dst, 6, 20);  // TCP
    let udp_csum = pseudo_header_checksum(src, dst, 17, 20); // UDP
    test_assert!(tcp_csum != udp_csum, "pseudo_header_checksum TCP != UDP");

    // Test 12: Large data checksum
    let large = [0xDEu8; 100];
    let csum = ip_checksum(&large);
    // With all 0xDE bytes, pairs are 0xDEDE = 57054, sum = 50 * 57054 = 2852700
    // 2852700 = 0x2B8B2C, fold: 0x8B2C + 0x2 = 0x8B2E, complement: 0x74D1
    test_assert!(csum != 0, "ip_checksum large data non-zero");
}
