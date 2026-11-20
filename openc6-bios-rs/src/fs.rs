//! 日志式环形 Flash 文件系统（openc6_fs）。
//!
//! 所有写入都追加到环尾，删除只是追加一条 `state = 0` 的墓碑记录；
//! 空间不足时按扇区做一次垃圾回收，把该扇区仍存活的块搬走后整扇区擦除。

use alloc::{string::String, vec, vec::Vec};
use core::cell::RefCell;

use critical_section::Mutex;

use crate::hal::{console, flash};

/// 扇区大小。
const SECTOR_SIZE: u32 = 4096;
/// 记录魔数。
const MAGIC: u16 = 0xC6F5;
/// 扇区填充标记，`total_len` 是整段填充长度。
const MAGIC_SKIP: u16 = 0xEEEE;
/// 文件节点类型。
pub const TYPE_FILE: u8 = 0x01;
/// 目录节点类型。
pub const TYPE_DIR: u8 = 0x02;
/// 记录头字节数。
const HEADER_SIZE: u32 = 32;
/// 单条记录的最大负载字节数。
const MAX_PAYLOAD: u32 = SECTOR_SIZE - HEADER_SIZE;
/// 文件名最大字节数（含结尾 0）。
const NAME_MAX: usize = 18;
/// 索引节点上限。
const MAX_NODES: usize = 1024;
/// 空槽标记。
const NONE: u32 = 0xFFFF_FFFF;

/// flash 上的记录头，32 字节小端。
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RecordHeader {
    magic: u16,
    total_len: u16,
    data_len: u16,
    id: u16,
    parent_id: u16,
    state: u8,
    type_: u8,
    chunk_idx: u16,
    name: [u8; NAME_MAX],
}

impl RecordHeader {
    fn parse(bytes: &[u8; HEADER_SIZE as usize]) -> Self {
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, HEADER_SIZE as usize)
        }
    }
}

/// 内存里的目录索引节点。
#[derive(Clone, Copy)]
pub struct Node {
    /// 节点 ID。
    pub id: u16,
    /// 父目录 ID。
    pub parent_id: u16,
    /// [`TYPE_FILE`] 或 [`TYPE_DIR`]。
    pub type_: u8,
    /// 文件名。
    pub name: [u8; NAME_MAX],
    /// 文件字节数，目录恒为 0。
    pub size: u32,
}

impl Node {
    /// 文件名字符串视图。
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// 文件系统全部可变状态。
struct Fs {
    nodes: Vec<Node>,
    head: u32,
    tail: u32,
    next_id: u16,
    cached_file_id: u16,
    chunk_cache: Vec<u32>,
}

static FS: Mutex<RefCell<Option<Fs>>> = Mutex::new(RefCell::new(None));

/// 在文件系统状态上执行一段操作。
fn with<T>(f: impl FnOnce(&mut Fs) -> T) -> Option<T> {
    critical_section::with(|cs| {
        let mut guard = FS.borrow_ref_mut(cs);
        guard.as_mut().map(f)
    })
}

/// 按字节偏移读 flash。
fn read_u32(addr: u32) -> u32 {
    let mut buf = [0u8; 4];
    flash::fs_read(addr, &mut buf);
    u32::from_le_bytes(buf)
}

/// 读记录头。
fn read_header(addr: u32) -> RecordHeader {
    let mut buf = [0u8; HEADER_SIZE as usize];
    flash::fs_read(addr, &mut buf);
    RecordHeader::parse(&buf)
}

impl Fs {
    fn fs_size(&self) -> u32 {
        flash::fs_size()
    }

    fn find_idx(&self, id: u16) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    fn upsert(&mut self, id: u16, parent_id: u16, type_: u8, name: &str, size: u32) {
        if let Some(idx) = self.find_idx(id) {
            self.nodes[idx].size = size;
        } else {
            if self.nodes.len() >= MAX_NODES {
                console::print("[FS] 索引节点已满\r\n");
                return;
            }
            let mut name_bytes = [0u8; NAME_MAX];
            let n = name.len().min(NAME_MAX - 1);
            name_bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
            self.nodes.push(Node {
                id,
                parent_id,
                type_,
                name: name_bytes,
                size,
            });
        }
        if id >= self.next_id {
            self.next_id = id + 1;
        }
    }

    fn remove(&mut self, id: u16) {
        if let Some(idx) = self.find_idx(id) {
            self.nodes.swap_remove(idx);
        }
    }

    fn free_space(&self) -> u32 {
        let total = self.fs_size();
        if self.head < self.tail {
            self.tail - self.head
        } else if self.head > self.tail {
            total - self.head + self.tail
        } else {
            total
        }
    }

    /// 把剩余空间填一条 skip 记录，对齐到下一个扇区。
    fn pad_sector(&mut self) {
        let remain = SECTOR_SIZE - (self.head % SECTOR_SIZE);
        if remain == SECTOR_SIZE {
            return;
        }
        if remain >= 4 {
            let mut buf = [0u8; 4];
            buf[..2].copy_from_slice(&MAGIC_SKIP.to_le_bytes());
            buf[2..].copy_from_slice(&(remain as u16).to_le_bytes());
            flash::fs_write(self.head, &buf);
        }
        self.head = (self.head + remain) % self.fs_size();
    }

    /// 重建某个文件的块地址缓存。
    fn build_cache(&mut self, id: u16) {
        if self.cached_file_id == id && !self.chunk_cache.is_empty() {
            return;
        }

        let Some(idx) = self.find_idx(id) else { return };
        let file_size = self.nodes[idx].size;

        self.cached_file_id = NONE as u16;
        self.chunk_cache.clear();

        let needed = if file_size == 0 {
            1
        } else {
            ((file_size + MAX_PAYLOAD - 1) / MAX_PAYLOAD) as usize
        };
        self.chunk_cache = vec![NONE; needed];
        self.cached_file_id = id;

        let total = self.fs_size();
        let mut curr = self.tail;

        while curr != self.head {
            let magic = read_u32(curr) as u16;
            let tlen = (read_u32(curr) >> 16) as u16;

            if magic == MAGIC_SKIP {
                curr = (curr + tlen as u32) % total;
                continue;
            }
            if magic != MAGIC {
                break;
            }

            let hdr = read_header(curr);
            if hdr.id == id {
                if hdr.state == 1 {
                    if (hdr.chunk_idx as usize) < self.chunk_cache.len() {
                        self.chunk_cache[hdr.chunk_idx as usize] = curr;
                    }
                } else {
                    self.chunk_cache.fill(NONE);
                }
            }
            curr = (curr + tlen as u32) % total;
        }
    }

    /// 原样追加一条记录，返回落盘地址。
    fn append_raw(&mut self, hdr: &RecordHeader, data: &[u8]) -> u32 {
        let remain = SECTOR_SIZE - (self.head % SECTOR_SIZE);
        if hdr.total_len as u32 > remain {
            self.pad_sector();
        }

        let addr = self.head;
        flash::fs_write(addr, hdr.as_bytes());
        if !data.is_empty() {
            flash::fs_write(addr + HEADER_SIZE, data);
        }
        self.head = (self.head + hdr.total_len as u32) % self.fs_size();
        addr
    }

    /// 回收环头一个扇区：搬走仍存活的块，再擦掉整个扇区。
    fn gc_step(&mut self) {
        let sector_start = (self.tail / SECTOR_SIZE) * SECTOR_SIZE;
        let mut curr = sector_start;

        while curr < sector_start + SECTOR_SIZE {
            let magic = read_u32(curr) as u16;
            let tlen = (read_u32(curr) >> 16) as u16;

            if tlen == 0 || tlen as u32 > SECTOR_SIZE {
                break;
            }
            if magic == MAGIC_SKIP {
                curr += tlen as u32;
                continue;
            }
            if magic != MAGIC {
                break;
            }

            let hdr = read_header(curr);
            if hdr.state == 1 && self.find_idx(hdr.id).is_some() {
                self.build_cache(hdr.id);
                if (hdr.chunk_idx as usize) < self.chunk_cache.len()
                    && self.chunk_cache[hdr.chunk_idx as usize] == curr
                {
                    let mut buf = vec![0u8; hdr.data_len as usize];
                    flash::fs_read(curr + HEADER_SIZE, &mut buf);
                    let new_addr = self.append_raw(&hdr, &buf);
                    self.chunk_cache[hdr.chunk_idx as usize] = new_addr;
                }
            }
            curr += tlen as u32;
        }

        flash::fs_erase_sector(sector_start / SECTOR_SIZE);
        self.tail = (sector_start + SECTOR_SIZE) % self.fs_size();
    }

    fn ensure_space(&mut self, needed: u32) {
        while self.free_space() < needed + SECTOR_SIZE * 3 {
            self.gc_step();
        }
    }

    /// 清空整个分区。
    fn format(&mut self) {
        let sectors = self.fs_size() / SECTOR_SIZE;
        console::print("[FS] 正在格式化分区...\r\n");

        // 擦除期间不往控制台回显，避免和 flash 缓存抢总线
        console::set_tap(None);

        for i in 0..sectors {
            flash::fs_erase_sector(i);
            if i % 128 == 0 || i == sectors - 1 {
                defmt::info!("[FS] 擦除进度 {}%", (i + 1) * 100 / sectors);
            }
        }

        self.head = 0;
        self.tail = 0;
        self.nodes.clear();
        self.next_id = 1;
        self.cached_file_id = NONE as u16;
        self.chunk_cache.clear();
        self.push_root();

        console::print("[FS] 格式化完成\r\n");
    }

    /// 挂载：从 flash 重建环指针与目录索引。
    fn mount(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.nodes.clear();
        self.next_id = 1;
        self.cached_file_id = NONE as u16;
        self.chunk_cache.clear();
        self.push_root();

        let sectors = self.fs_size() / SECTOR_SIZE;
        let mut gap_start = NONE;
        let mut gap_end = NONE;
        let mut has_data = false;

        for i in 0..sectors {
            let current = read_u32(i * SECTOR_SIZE);
            let next = read_u32(((i + 1) % sectors) * SECTOR_SIZE);

            if current != 0xFFFF_FFFF {
                has_data = true;
            }
            if current != 0xFFFF_FFFF && next == 0xFFFF_FFFF {
                gap_start = (i + 1) % sectors;
            }
            if current == 0xFFFF_FFFF && next != 0xFFFF_FFFF {
                gap_end = i;
            }
        }

        if !has_data {
            defmt::warn!("[FS] 分区全空，初始化为空环");
            return;
        }

        if gap_start == NONE {
            defmt::warn!("[FS] 环形边界损坏，自动格式化");
            self.format();
            return;
        }

        self.tail = ((gap_end + 1) % sectors) * SECTOR_SIZE;
        let search_sec = if gap_start == 0 { sectors - 1 } else { gap_start - 1 };
        self.head = search_sec * SECTOR_SIZE;

        loop {
            let magic = read_u32(self.head) as u16;
            if magic == 0xFFFF {
                break;
            }
            let tlen = (read_u32(self.head) >> 16) as u16;
            if tlen == 0 || tlen as u32 > SECTOR_SIZE {
                break;
            }
            self.head = (self.head + tlen as u32) % self.fs_size();
            if self.head % SECTOR_SIZE == 0 {
                break;
            }
        }

        let mut curr = self.tail;
        while curr != self.head {
            let magic = read_u32(curr) as u16;
            let tlen = (read_u32(curr) >> 16) as u16;

            if magic == MAGIC_SKIP {
                curr = (curr + tlen as u32) % self.fs_size();
                continue;
            }
            if magic != MAGIC {
                break;
            }

            let hdr = read_header(curr);
            if hdr.state == 0 {
                self.remove(hdr.id);
            } else {
                let current_size = hdr.chunk_idx as u32 * MAX_PAYLOAD + hdr.data_len as u32;
                match self.find_idx(hdr.id) {
                    Some(idx) => {
                        if current_size > self.nodes[idx].size {
                            self.nodes[idx].size = current_size;
                        }
                    }
                    None => {
                        let name = {
                            let end = hdr.name.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
                            core::str::from_utf8(&hdr.name[..end]).unwrap_or("")
                        };
                        self.upsert(hdr.id, hdr.parent_id, hdr.type_, name, current_size);
                    }
                }
            }
            curr = (curr + tlen as u32) % self.fs_size();
        }

        defmt::info!(
            "[FS] 已挂载 {} 个节点 (head={:#x}, tail={:#x})",
            self.nodes.len(),
            self.head,
            self.tail
        );
    }

    /// 把根目录作为 id 0 的节点放进索引。
    ///
    /// 根目录没有对应的 flash 记录，不显式塞进来 `get_type(0)` 会返回 0，
    /// 导致 `cd /` 被判成「不是目录」。`parent_id` 用 NONE 保证它不会出现在自己的列表里。
    fn push_root(&mut self) {
        self.nodes.push(Node {
            id: 0,
            parent_id: NONE as u16,
            type_: TYPE_DIR,
            name: [0; NAME_MAX],
            size: 0,
        });
    }

    fn find_id(&self, name: &str, parent_id: u16) -> Option<u16> {
        self.nodes
            .iter()
            .find(|n| n.parent_id == parent_id && n.name_str() == name)
            .map(|n| n.id)
    }

    fn mkdir(&mut self, name: &str, parent_id: u16) -> u16 {
        if let Some(id) = self.find_id(name, parent_id) {
            return id;
        }

        let id = self.next_id;
        self.next_id += 1;

        let padded_len = (HEADER_SIZE + 3) & !3;
        self.ensure_space(padded_len);

        if padded_len > SECTOR_SIZE - (self.head % SECTOR_SIZE) {
            self.pad_sector();
        }

        let mut hdr = RecordHeader {
            magic: MAGIC,
            total_len: padded_len as u16,
            data_len: 0,
            id,
            parent_id,
            state: 1,
            type_: TYPE_DIR,
            chunk_idx: 0,
            name: [0; NAME_MAX],
        };
        let n = name.len().min(NAME_MAX - 1);
        hdr.name[..n].copy_from_slice(&name.as_bytes()[..n]);

        flash::fs_write(self.head, hdr.as_bytes());
        self.head = (self.head + padded_len) % self.fs_size();

        self.upsert(id, parent_id, TYPE_DIR, name, 0);
        id
    }

    fn delete(&mut self, id: u16) {
        if self.find_idx(id).is_none() {
            return;
        }

        let padded_len = (HEADER_SIZE + 3) & !3;
        self.ensure_space(padded_len);

        if padded_len > SECTOR_SIZE - (self.head % SECTOR_SIZE) {
            self.pad_sector();
        }

        let hdr = RecordHeader {
            magic: MAGIC,
            total_len: padded_len as u16,
            data_len: 0,
            id,
            state: 0,
            ..Default::default()
        };

        flash::fs_write(self.head, hdr.as_bytes());
        self.head = (self.head + padded_len) % self.fs_size();

        self.remove(id);
        if self.cached_file_id == id {
            self.cached_file_id = NONE as u16;
            self.chunk_cache.clear();
        }
    }

    fn write_file(&mut self, name: &str, data: &[u8], parent_id: u16) -> u16 {
        let id = match self.find_id(name, parent_id) {
            Some(existing) => {
                self.delete(existing);
                existing
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                id
            }
        };

        let mut offset = 0usize;
        let mut chunk_idx = 0u16;

        loop {
            let chunk_len = (data.len() - offset).min(MAX_PAYLOAD as usize);
            let padded_len = (HEADER_SIZE + chunk_len as u32 + 3) & !3;

            if padded_len > SECTOR_SIZE - (self.head % SECTOR_SIZE) {
                self.pad_sector();
            }
            self.ensure_space(padded_len);

            let mut hdr = RecordHeader {
                magic: MAGIC,
                total_len: padded_len as u16,
                data_len: chunk_len as u16,
                id,
                parent_id,
                state: 1,
                type_: TYPE_FILE,
                chunk_idx,
                name: [0; NAME_MAX],
            };
            let n = name.len().min(NAME_MAX - 1);
            hdr.name[..n].copy_from_slice(&name.as_bytes()[..n]);

            flash::fs_write(self.head, hdr.as_bytes());
            if chunk_len > 0 {
                flash::fs_write(self.head + HEADER_SIZE, &data[offset..offset + chunk_len]);
            }
            self.head = (self.head + padded_len) % self.fs_size();

            offset += chunk_len;
            chunk_idx += 1;
            if offset >= data.len() {
                break;
            }
        }

        self.upsert(id, parent_id, TYPE_FILE, name, data.len() as u32);
        if self.cached_file_id == id {
            self.cached_file_id = NONE as u16;
            self.chunk_cache.clear();
        }
        id
    }

    fn read_file(&mut self, id: u16, offset: u32, len: u32, dest: &mut [u8]) -> i32 {
        let Some(idx) = self.find_idx(id) else {
            return -1;
        };
        if self.nodes[idx].type_ != TYPE_FILE {
            return -1;
        }

        let file_size = self.nodes[idx].size;
        if offset >= file_size {
            return 0;
        }
        let len = len.min(file_size - offset).min(dest.len() as u32);

        self.build_cache(id);
        if self.chunk_cache.is_empty() {
            return -1;
        }

        let start_chunk = offset / MAX_PAYLOAD;
        let end_chunk = (offset + len - 1) / MAX_PAYLOAD;
        let mut read_count = 0usize;

        for c in start_chunk..=end_chunk {
            if c as usize >= self.chunk_cache.len() {
                break;
            }
            let addr = self.chunk_cache[c as usize];
            if addr == NONE {
                continue;
            }

            let hdr = read_header(addr);
            let chunk_offset = if c == start_chunk { offset % MAX_PAYLOAD } else { 0 };
            let mut bytes_to_read = hdr.data_len as u32 - chunk_offset;
            if read_count as u32 + bytes_to_read > len {
                bytes_to_read = len - read_count as u32;
            }

            flash::fs_read(
                addr + HEADER_SIZE + chunk_offset,
                &mut dest[read_count..read_count + bytes_to_read as usize],
            );
            read_count += bytes_to_read as usize;
        }

        read_count as i32
    }
}

/// 挂载文件系统；分区不可用时什么也不做。
pub fn init() {
    if !flash::is_ready() {
        defmt::error!("[FS] openc6_fs 分区不可用");
        return;
    }

    let mut fs = Fs {
        nodes: Vec::new(),
        head: 0,
        tail: 0,
        next_id: 1,
        cached_file_id: NONE as u16,
        chunk_cache: Vec::new(),
    };
    fs.mount();

    critical_section::with(|cs| *FS.borrow_ref_mut(cs) = Some(fs));
}

/// 擦掉整个分区，重建空环。
pub fn format() {
    with(|fs| fs.format());
}

/// 建目录，返回节点 ID。
pub fn mkdir(name: &str, parent_id: u16) -> Option<u16> {
    with(|fs| fs.mkdir(name, parent_id))
}

/// 写文件，覆盖同名文件，返回节点 ID。
pub fn write_file(name: &str, data: &[u8], parent_id: u16) -> Option<u16> {
    with(|fs| fs.write_file(name, data, parent_id))
}

/// 按 ID 读文件，返回实际读到的字节数。
pub fn read_file(id: u16, offset: u32, len: u32, dest: &mut [u8]) -> i32 {
    with(|fs| fs.read_file(id, offset, len, dest)).unwrap_or(-1)
}

/// 读整个文件到 `Vec`。
pub fn read_whole(id: u16) -> Option<Vec<u8>> {
    let size = get_size(id)?;
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let n = read_file(id, 0, size as u32, &mut buf);
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(buf)
}

/// 删除节点。
pub fn delete(id: u16) {
    with(|fs| fs.delete(id));
}

/// 在父目录里按名字找 ID。
pub fn find_id(name: &str, parent_id: u16) -> Option<u16> {
    with(|fs| fs.find_id(name, parent_id)).flatten()
}

/// 列出目录下的所有节点。
pub fn list_dir(parent_id: u16) -> Vec<Node> {
    with(|fs| {
        fs.nodes
            .iter()
            .filter(|n| n.parent_id == parent_id)
            .copied()
            .collect()
    })
    .unwrap_or_default()
}

/// 节点字节数，目录返回 0，找不到返回 `None`。
pub fn get_size(id: u16) -> Option<i32> {
    with(|fs| {
        fs.find_idx(id)
            .map(|idx| fs.nodes[idx].size as i32)
    })
    .flatten()
}

/// 节点类型，找不到返回 0。
pub fn get_type(id: u16) -> u8 {
    with(|fs| {
        fs.find_idx(id)
            .map(|idx| fs.nodes[idx].type_)
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// 节点的父目录 ID，根返回 0。
/// 路径解析：从根开始按 `/` 逐级查找，返回节点 ID。
/// 把 `path` 拆成（父目录 ID, 文件名）。
///
/// 写操作需要的是「在哪个目录下、叫什么名字」，直接拿整条路径当文件名会把
/// `mkdir /t` 存成名字 `/t`，之后任何按 `t` 的查找都找不到。
pub fn split_path<'a>(path: &'a str, cwd: u16) -> Option<(u16, &'a str)> {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    };
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let parent = if dir.is_empty() {
        if path.starts_with('/') { 0 } else { cwd }
    } else {
        resolve(dir, cwd)?
    };
    Some((parent, name))
}

pub fn resolve(path: &str, cwd: u16) -> Option<u16> {
    let mut current = if path.starts_with('/') { 0 } else { cwd };
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        current = find_id(part, current)?;
    }
    Some(current)
}

/// 取节点名字，供 shell 拼路径。
pub fn name_of(id: u16) -> String {
    with(|fs| {
        fs.find_idx(id)
            .map(|idx| String::from(fs.nodes[idx].name_str()))
            .unwrap_or_default()
    })
    .unwrap_or_default()
}
