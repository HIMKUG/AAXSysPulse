//! 静态系统信息：内存条数（SMBIOS type 17）+ 物理磁盘数。
//!
//! 整机功耗估算用（用户拍板：内存每条 5W、主板 44W、每块硬盘 6W）。
//! 都是准静态量：进程启动时读一次即可。

use crate::probe;

/// 内存条数：解析 SMBIOS type 17（Memory Device），`Size != 0` 即为已装条。
pub fn ram_stick_count() -> usize {
    unsafe {
        use windows::Win32::System::SystemInformation::{
            GetSystemFirmwareTable, FIRMWARE_TABLE_PROVIDER,
        };
        // 'RSMB' = Raw SMBIOS。
        // Windows API 的 provider ID 是大端序多字符常量：'RSMB' = 0x52534D42。
        // 用 from_le_bytes 会得到 0x424D5352（字节序反了），API 返回 0，
        // 实测即 `smbios size=0`、内存条数恒为 0 的根因。
        let sig = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
        let size = GetSystemFirmwareTable(sig, 0, None);
        if size == 0 {
            probe::log("sysinfo smbios size=0");
            return 0;
        }
        let mut buf = vec![0u8; size as usize];
        let got = GetSystemFirmwareTable(sig, 0, Some(&mut buf));
        if got == 0 {
            return 0;
        }
        // RawSMBIOSData: UsedCallingMethod(1) MajorVer(1) MinorVer(1) DmiRevision(1) Length(4) 数据
        if buf.len() < 8 {
            return 0;
        }
        let len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let end = (8 + len).min(buf.len());
        let data = &buf[8..end];

        let mut sticks = 0usize;
        let mut i = 0usize;
        while i + 4 <= data.len() {
            let stype = data[i];
            let slen = data[i + 1] as usize;
            if slen < 4 || i + slen > data.len() {
                break;
            }
            if stype == 17 && slen >= 0x0E {
                // SMBIOS type 17（Memory Device）布局：
                //   0x00 Type(1) 0x01 Length(1) 0x02 Handle(2)
                //   0x04 PhysMemArrayHandle(2) 0x06 MemErrInfoHandle(2)
                //   0x08 TotalWidth(2) 0x0A DataWidth(2)
                //   0x0C Size(2) ← 容量；bit15=单位(0=MB,1=KB)
                // 0 = 槽位未装，0xFFFF = 容量未知，均不计入。
                let size_raw = u16::from_le_bytes([data[i + 0x0C], data[i + 0x0D]]);
                let total_w = u16::from_le_bytes([data[i + 8], data[i + 9]]);
                if probe::enabled() {
                    probe::log(&format!(
                        "sysinfo t17 slen={slen} size_raw=0x{size_raw:04x} total_width={total_w}"
                    ));
                }
                if size_raw != 0 && size_raw != 0xFFFF {
                    sticks += 1;
                }
            }
            // 跳过字符串区（双 \0 结束）
            i += slen;
            while i + 1 < data.len() && !(data[i] == 0 && data[i + 1] == 0) {
                i += 1;
            }
            i += 2;
        }
        if probe::enabled() {
            probe::log(&format!(
                "sysinfo ram_sticks={sticks} smbios_bytes={}",
                data.len()
            ));
        }
        sticks.min(64)
    }
}

/// 物理磁盘数：枚举 \\.\PhysicalDriveN（0 访问权限打开即算存在）。
pub fn physical_disk_count() -> usize {
    let mut n = 0usize;
    for i in 0..32 {
        let path = format!("\\\\.\\PhysicalDrive{i}\0");
        let wide: Vec<u16> = path.encode_utf16().collect();
        unsafe {
            let h = windows::Win32::Storage::FileSystem::CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                0, // 仅查询存在性，不需要读写
                windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                None,
                windows::Win32::Storage::FileSystem::OPEN_EXISTING,
                windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
                None,
            );
            if let Ok(hd) = h {
                if !hd.is_invalid() {
                    n += 1;
                }
                let _ = windows::Win32::Foundation::CloseHandle(hd);
            }
        }
    }
    if probe::enabled() {
        probe::log(&format!("sysinfo physical_disks={n}"));
    }
    n.min(32)
}
