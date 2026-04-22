//! Unicorn-engine-backed stub emulator.
//!
//! Ported line-by-line from Mowokuma's `src/emulator/stub_emulator.rs`
//! (commit 7181c9a). The memory layout (stack / heap addresses, sizes,
//! page alignment), the three function patches (skip-stub and two
//! allocator shellcodes), the 91-byte x86-64 allocator shellcode itself,
//! and the mem-hook logic that pulls ward-spawn fields out at specific
//! write-counts are all her work. See CREDITS.md.
//!
//! The shellcode encodes two addresses literally in its bytes:
//!   - offset 0x16: 0x00007FF76AFD0000 (= Config::DEFAULT_BASE_ADDR)
//!   - offset 0x22: 0x00007FFFFFFF8000 (= HEAP_BASE)
//! If either address ever changes, the shellcode must be regenerated.

use std::sync::{Arc, Mutex};

use unicorn_engine::{
    ffi::uc_strerror,
    uc_error,
    unicorn_const::{Arch as UcArch, Mode as UcMode},
    HookType, Permission, RegisterX86, Unicorn,
};

use crate::emulator::config::{Config, Section};
use crate::emulator::packet::{PathPacket, WardSpawnPacket};
use crate::error::{Result, RoflError};

/// Wraps a Unicorn VM configured to run the game client's own decrypt
/// functions on packet payloads.
///
/// One instance is intended to be reused across a batch of packets of the
/// same type: call `setup()` once, then loop calling `setup_args()` /
/// `call_decrypt_*()` / `reset()` per packet.
pub struct StubEmulator<'a> {
    config: Config,
    uc: Unicorn<'a, ()>,
    packet_addr: u64,
    packet_size: usize,
}

impl<'a> StubEmulator<'a> {
    const PAGE_SIZE: usize = 0x1000;

    const STACK_BASE: u64 = 0x7FFF_FFFF_0000;
    const STACK_SIZE: usize = 0x2000;

    const HEAP_BASE: u64 = 0x7FFF_FFFF_8000;
    const HEAP_SIZE: usize = 0x2000;

    const HEAP_CURSOR_PTR: u64 = 0x0;

    pub fn new(config: Config) -> Self {
        let uc = Unicorn::new(UcArch::X86, UcMode::MODE_64)
            .expect("Unicorn::new failed for x86_64");
        Self {
            config,
            uc,
            packet_addr: 0,
            packet_size: 0,
        }
    }

    /// Map stack, heap, and the three PE sections into the VM; patch the
    /// allocator and safety-check functions with inline shellcode.
    /// Call once per `StubEmulator` instance before any packet decodes.
    pub fn setup(&mut self) -> Result<()> {
        self.map_stack()?;
        self.map_heap()?;
        self.map_sections()?;
        self.patch_functions()?;
        Ok(())
    }

    /// Place a packet's payload bytes into the VM's heap and set up the
    /// registers that the decrypt function expects as its arguments.
    ///
    /// Windows x64 calling convention: `RCX` = first arg (the output
    /// packet struct pointer), `RDX` = second arg (a pointer to a pointer
    /// to the payload), `R8` = third arg (the payload-end pointer).
    pub fn setup_args(&mut self, payload: &[u8]) -> Result<()> {
        self.packet_size = 0x90;
        self.packet_addr = self.alloc(self.packet_size);

        let ptr = self.alloc_and_store(payload)?;
        let payload_ptr = self.alloc_and_store(&ptr.to_le_bytes())?;
        let payload_end = ptr + payload.len() as u64;

        self.write_reg(RegisterX86::RCX, self.packet_addr)?;
        self.write_reg(RegisterX86::RDX, payload_ptr)?;
        self.write_reg(RegisterX86::R8, payload_end)?;
        Ok(())
    }

    /// Reset the bump heap cursor and stack pointer so the next packet
    /// starts from a clean state. The mapped PE sections and the allocator
    /// patches persist across resets.
    pub fn reset(&mut self) -> Result<()> {
        self.set_heap_cursor(0)?;
        self.write_reg(
            RegisterX86::RSP,
            Self::STACK_BASE + (Self::STACK_SIZE - 0x100) as u64,
        )
    }

    /// Call the ward-spawn decrypt function. Installs a mem-write hook
    /// over the output packet struct, captures `x`, `y`, `id`, `owner_id`
    /// at the per-field write count documented in the config, then reads
    /// the name pointer + length out of the struct when the function
    /// returns.
    pub fn call_decrypt_ward_spawn_packet(
        &mut self,
        call_rva: u64,
        end_rva: u64,
        timestamp: f32,
    ) -> Result<WardSpawnPacket> {
        let packet_addr_clone = self.packet_addr;
        let packet_size = self.packet_size;

        let x = Arc::new(Mutex::new(0.0f32));
        let y = Arc::new(Mutex::new(0.0f32));
        let id = Arc::new(Mutex::new(0u32));
        let owner_id = Arc::new(Mutex::new(0u32));

        let x_clone = Arc::clone(&x);
        let y_clone = Arc::clone(&y);
        let id_clone = Arc::clone(&id);
        let owner_id_clone = Arc::clone(&owner_id);

        let x_offset = self.config.ward_spawn_decrypt.x_offset as u16;
        let x_write_count = self.config.ward_spawn_decrypt.x_write_count as usize;
        let y_offset = self.config.ward_spawn_decrypt.y_offset as u16;
        let y_write_count = self.config.ward_spawn_decrypt.y_write_count as usize;
        let id_offset = self.config.ward_spawn_decrypt.id_offset as u16;
        let owner_id_offset = self.config.ward_spawn_decrypt.owner_id_offset as u16;

        let mut packet_write_count: Vec<usize> = vec![0; packet_size];

        self.uc
            .add_mem_hook(
                HookType::MEM_WRITE,
                self.packet_addr,
                self.packet_addr + packet_size as u64,
                move |_uc, _type, addr, size, value| {
                    if size == 1 {
                        return false;
                    }

                    let offset = (addr - packet_addr_clone) as u16;
                    let count = packet_write_count[offset as usize];

                    if offset == x_offset && count == x_write_count {
                        *x_clone.lock().unwrap() = f32::from_bits(value as u32);
                    }
                    if offset == y_offset && count == y_write_count {
                        *y_clone.lock().unwrap() = f32::from_bits(value as u32);
                    }
                    if offset == id_offset && count == 0 {
                        *id_clone.lock().unwrap() = value as u32;
                    }
                    if offset == owner_id_offset && count == 0 {
                        *owner_id_clone.lock().unwrap() = value as u32;
                    }

                    for i in offset..offset + size as u16 {
                        if (i as usize) < packet_write_count.len() {
                            packet_write_count[i as usize] += 1;
                        }
                    }

                    size > 1 || count == 0
                },
            )
            .map_err(|e| uc_setup_err("ward-spawn mem hook", e))?;

        let _ = self.uc.emu_start(
            self.rva_to_address(call_rva),
            self.rva_to_address(end_rva),
            0,
            0,
        );

        let x = *x.lock().unwrap();
        let y = *y.lock().unwrap();
        let id = *id.lock().unwrap();
        let owner_id = *owner_id.lock().unwrap();

        let ptr = self.read_ptr_on(
            self.packet_addr + self.config.ward_spawn_decrypt.name_offset,
        )?;
        let size = self.read_u32_on(
            self.packet_addr + self.config.ward_spawn_decrypt.name_len_offset,
        )?;
        let name = self.read_str_on(ptr, size as usize)?;

        Ok(WardSpawnPacket {
            timestamp,
            name,
            id,
            owner_id,
            x: x as i32,
            y: y as i32,
        })
    }

    /// Call a small function with only `this` passed in RCX and read
    /// back the u64 return value from RAX. Used to probe candidate
    /// "get-my-netid" or "get-my-typeid" callbacks on class descriptor
    /// entries. Caller provides a stop RVA (typically the function's
    /// `ret` instruction address or slightly past); on timeout or bad
    /// fetch we return `None` rather than panic.
    pub fn call_thiscall_returning_u64(
        &mut self,
        call_rva: u64,
        end_rva: u64,
        this_ptr: u64,
    ) -> Option<u64> {
        // Fresh stack pointer, clean heap cursor.
        if self.set_heap_cursor(0).is_err() {
            return None;
        }
        if self
            .write_reg(
                RegisterX86::RSP,
                Self::STACK_BASE + (Self::STACK_SIZE - 0x100) as u64,
            )
            .is_err()
        {
            return None;
        }
        if self.write_reg(RegisterX86::RCX, this_ptr).is_err() {
            return None;
        }
        // Cap instruction count aggressively; these "get netid" thunks
        // should be a handful of instructions. Budget 512 insts.
        let _ = self.uc.emu_start(
            self.rva_to_address(call_rva),
            self.rva_to_address(end_rva),
            0,
            512,
        );
        self.uc.reg_read(RegisterX86::RAX).ok()
    }

    /// Trace every memory write inside the output-struct range while
    /// executing a decoder function. Used to discover struct layouts of
    /// unknown decoders, and to verify that a hypothesised decoder RVA
    /// actually produces structured writes (rather than crashing or
    /// touching random memory).
    ///
    /// Returns a Vec of (offset_in_struct, size_in_bytes, value) in the
    /// exact order the writes occurred. Writes to offsets outside the
    /// 0x90-byte hooked range are ignored.
    pub fn trace_decoder_writes(
        &mut self,
        call_rva: u64,
        end_rva: u64,
    ) -> Result<Vec<(u16, u8, u64)>> {
        let packet_addr = self.packet_addr;
        let packet_size = self.packet_size;
        let log: Arc<Mutex<Vec<(u16, u8, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = Arc::clone(&log);

        self.uc
            .add_mem_hook(
                HookType::MEM_WRITE,
                packet_addr,
                packet_addr + packet_size as u64,
                move |_uc, _type, addr, size, value| {
                    let offset = (addr - packet_addr) as u16;
                    log_clone
                        .lock()
                        .unwrap()
                        .push((offset, size as u8, value as u64));
                    true
                },
            )
            .map_err(|e| uc_setup_err("trace mem hook", e))?;

        let _ = self.uc.emu_start(
            self.rva_to_address(call_rva),
            self.rva_to_address(end_rva),
            0,
            0,
        );

        let out = log.lock().unwrap().clone();
        Ok(out)
    }

    /// Call the movement-packet decrypt function, read back the pointer
    /// and size of the decoded payload from the output struct, and parse
    /// it into a `PathPacket`.
    pub fn call_decrypt_pos_packet(
        &mut self,
        call_rva: u64,
        end_rva: u64,
        timestamp: f32,
    ) -> Result<PathPacket> {
        self.write_reg(
            RegisterX86::RSP,
            Self::STACK_BASE + (Self::STACK_SIZE - 0x100) as u64,
        )?;

        let _ = self.uc.emu_start(
            self.rva_to_address(call_rva),
            self.rva_to_address(end_rva),
            0,
            0,
        );

        let size = self.read_u32_on(
            self.packet_addr + self.config.mov_decrypt.payload_size_offset,
        )?;
        let ptr = self.read_ptr_on(
            self.packet_addr + self.config.mov_decrypt.payload_offset,
        )?;

        let payload = self.read_buffer_on(ptr, size as usize)?;
        PathPacket::parse(timestamp, payload)
    }

    fn map_stack(&mut self) -> Result<()> {
        self.uc
            .mem_map(
                Self::STACK_BASE,
                Self::STACK_SIZE,
                Permission::READ | Permission::WRITE,
            )
            .map_err(|e| uc_setup_err("map stack", e))?;
        self.write_reg(
            RegisterX86::RSP,
            Self::STACK_BASE + (Self::STACK_SIZE as u64 - 0x100),
        )
    }

    fn map_heap(&mut self) -> Result<()> {
        self.uc
            .mem_map(
                Self::HEAP_BASE,
                Self::HEAP_SIZE,
                Permission::READ | Permission::WRITE,
            )
            .map_err(|e| uc_setup_err("map heap", e))?;
        self.uc
            .mem_map(
                Self::align_addr(self.rva_to_address(Self::HEAP_CURSOR_PTR)),
                Self::align_size(4),
                Permission::READ | Permission::WRITE,
            )
            .map_err(|e| uc_setup_err("map heap cursor", e))?;
        self.set_heap_cursor(0)
    }

    fn map_sections(&mut self) -> Result<()> {
        self.map_section(&self.config.text.clone())?;
        self.map_section(&self.config.data.clone())?;
        self.map_section(&self.config.rdata.clone())?;
        Ok(())
    }

    fn map_section(&mut self, sect: &Section) -> Result<()> {
        let sect_addr = self.rva_to_address(sect.rva);
        self.uc
            .mem_map(
                Self::align_addr(sect_addr),
                Self::align_size(sect.size as usize),
                Permission::READ | Permission::WRITE | Permission::EXEC,
            )
            .map_err(|e| uc_setup_err(&format!("map .{}", sect.name), e))?;
        self.uc
            .mem_write(sect_addr, &sect.raw)
            .map_err(|e| uc_setup_err(&format!("write .{}", sect.name), e))?;
        Ok(())
    }

    fn patch_functions(&mut self) -> Result<()> {
        // `mov rax, 1; ret` : stubs a safety check to always succeed.
        let patch1 = [0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, 0xC3];

        // 91-byte hand-rolled bump allocator: saves caller registers,
        // loads the heap cursor from `[base_addr + HEAP_CURSOR_PTR]`,
        // computes `ptr = HEAP_BASE + cursor`, returns it in the packet
        // struct's first field, bumps the cursor by the requested size,
        // writes the cursor back, and restores registers.
        //
        // The literal addresses encoded inside this shellcode must match:
        //   offset 0x16: 0x00007FF76AFD0000 (config.base_addr)
        //   offset 0x22: 0x00007FFFFFFF8000 (HEAP_BASE)
        let patch2 = [
            0x53, 0x57, 0x56, 0x55, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53, 0x41, 0x54,
            0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0xB8, 0x00, 0x00, 0xFD, 0x6A, 0xF7, 0x7F,
            0x00, 0x00, 0x48, 0x8B, 0x18, 0x48, 0xB8, 0x00, 0x80, 0xFF, 0xFF, 0xFF, 0x7F, 0x00,
            0x00, 0x48, 0x8D, 0x04, 0x18, 0x48, 0x89, 0x01, 0x89, 0x51, 0x08, 0x01, 0xD3, 0x48,
            0xB8, 0x00, 0x00, 0xFD, 0x6A, 0xF7, 0x7F, 0x00, 0x00, 0x89, 0x18, 0x41, 0x5F, 0x41,
            0x5E, 0x41, 0x5D, 0x41, 0x5C, 0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5D,
            0x5E, 0x5F, 0x5B, 0xC3,
        ];

        self.uc
            .mem_write(self.rva_to_address(self.config.skip), &patch1)
            .map_err(|e| uc_setup_err("write skip-stub", e))?;
        self.uc
            .mem_write(self.rva_to_address(self.config.alloc1), &patch2)
            .map_err(|e| uc_setup_err("write alloc1 shellcode", e))?;
        self.uc
            .mem_write(self.rva_to_address(self.config.alloc2), &patch2)
            .map_err(|e| uc_setup_err("write alloc2 shellcode", e))?;
        Ok(())
    }

    fn get_heap_cursor(&mut self) -> Result<u64> {
        let offset = self.rva_to_address(Self::HEAP_CURSOR_PTR);
        let mut buf = [0u8; 8];
        self.uc
            .mem_read(offset, &mut buf)
            .map_err(|e| uc_runtime_err("read heap cursor", e))?;
        Ok(u64::from_le_bytes(buf))
    }

    fn set_heap_cursor(&mut self, cursor: u64) -> Result<()> {
        let offset = self.rva_to_address(Self::HEAP_CURSOR_PTR);
        self.uc
            .mem_write(offset, &cursor.to_le_bytes())
            .map_err(|e| uc_runtime_err("write heap cursor", e))
    }

    fn alloc(&mut self, size: usize) -> u64 {
        let cursor = self.get_heap_cursor().unwrap_or(0);
        let ptr = Self::HEAP_BASE + cursor;
        let _ = self.set_heap_cursor(cursor + size as u64);
        ptr
    }

    fn alloc_and_store(&mut self, data: &[u8]) -> Result<u64> {
        let ptr = self.alloc(data.len());
        self.uc
            .mem_write(ptr, data)
            .map_err(|e| uc_setup_err("store on heap", e))?;
        Ok(ptr)
    }

    fn write_reg(&mut self, reg: RegisterX86, value: u64) -> Result<()> {
        self.uc
            .reg_write(reg, value)
            .map_err(|e| uc_setup_err("reg_write", e))
    }

    fn read_str_on(&self, addr: u64, size: usize) -> Result<String> {
        let mut buf = vec![0u8; size];
        self.uc
            .mem_read(addr, &mut buf)
            .map_err(|e| uc_runtime_err("read string", e))?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn read_buffer_on(&self, addr: u64, size: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; size];
        self.uc
            .mem_read(addr, &mut buf)
            .map_err(|e| uc_runtime_err("read buffer", e))?;
        Ok(buf)
    }

    fn read_u32_on(&self, addr: u64) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.uc
            .mem_read(addr, &mut buf)
            .map_err(|e| uc_runtime_err("read u32", e))?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_ptr_on(&self, addr: u64) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.uc
            .mem_read(addr, &mut buf)
            .map_err(|e| uc_runtime_err("read ptr", e))?;
        Ok(u64::from_le_bytes(buf))
    }

    fn rva_to_address(&self, rva: u64) -> u64 {
        rva + self.config.base_addr
    }

    fn align_addr(addr: u64) -> u64 {
        addr & !(Self::PAGE_SIZE as u64 - 1)
    }

    fn align_size(size: usize) -> usize {
        (size + Self::PAGE_SIZE - 1) & !(Self::PAGE_SIZE - 1)
    }
}

fn uc_err_to_str(err: uc_error) -> String {
    unsafe {
        std::ffi::CStr::from_ptr(uc_strerror(err))
            .to_string_lossy()
            .into_owned()
    }
}

fn uc_setup_err(ctx: &str, err: uc_error) -> RoflError {
    RoflError::Io(std::io::Error::other(format!(
        "emulator setup ({ctx}): {}",
        uc_err_to_str(err)
    )))
}

fn uc_runtime_err(ctx: &str, err: uc_error) -> RoflError {
    RoflError::Io(std::io::Error::other(format!(
        "emulator runtime ({ctx}): {}",
        uc_err_to_str(err)
    )))
}
