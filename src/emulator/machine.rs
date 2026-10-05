//! A Unicorn x86-64 VM holding a League image plus just enough of a
//! Windows process around it for the packet deserializers to run.
//!
//! What the deserializers touch beyond their own code, and what stands in
//! for it here (details and evidence in `docs/DECODING.md`):
//!
//! - **Heap.** The game's operator new is patched to `ret` and trapped: a
//!   code hook hands out bump-allocated memory from `HEAP_BASE`. Its
//!   operator delete / free are patched to `ret`.
//! - **TLS.** Thread-safe statics read `_Init_thread_epoch` through
//!   `gs:[0x58]`. A fake TEB points at a TLS block copied from the image's
//!   TLS template.
//! - **Imports.** Riot's loader stub hides the import table, so on disk
//!   the IAT slots hold obfuscated values. Every slot listed in the
//!   archive is pointed at `xor eax, eax; ret`.
//! - **Control Flow Guard.** Indirect calls go through the CFG dispatch
//!   thunk (`jmp rax`); a hook there redirects targets outside mapped
//!   memory to the same stub (the CRT's encoded function-pointer cache is
//!   never initialised in a static image).
//! - **Team registry.** Enum readers range-check against a global built at
//!   session start; a stand-in object with the same count is installed.
//!
//! The VM also records, per heap byte, the last *plaintext* value written
//! to it. Deserializers decode a field, store it, then obfuscate it in
//! place with a byte-wise read-modify-write loop (Henry Zhu's
//! decrypt-access-release pattern). A one-byte write to a byte that was
//! read since its last write is that obfuscation pass and is not recorded;
//! every other write is.

use unicorn_engine::{
    unicorn_const::{Arch, HookType, MemType, Mode, Permission},
    RegisterX86, Unicorn,
};

use crate::emulator::archive::PatchArchive;
use crate::error::{Result, RoflError};

pub const STACK_BASE: u64 = 0x7FFF_0000_0000;
pub const STACK_SIZE: u64 = 0x10_0000;
pub const HEAP_BASE: u64 = 0x7FFE_0000_0000;
pub const HEAP_SIZE: u64 = 0x400_0000;
pub const STOP_ADDR: u64 = 0x7FFD_0000_0000;
pub const TEB_BASE: u64 = 0x7FFC_0000_0000;
pub const STUB_BASE: u64 = 0x7FFB_0000_0000;

const PAGE: u64 = 0x1000;
/// `xor eax, eax; ret`: every hidden import and every unresolvable
/// indirect call target lands here.
const RETURN_ZERO: [u8; 3] = [0x31, 0xC0, 0xC3];
/// Upper bound on instructions per call; deserializers run a few thousand.
const INSTRUCTION_LIMIT: usize = 5_000_000;
/// Translation-cache size per VM (see `Machine::new`).
const TCG_BUFFER_SIZE: u32 = 64 << 20;

const FLAG_READ: u8 = 1;
const FLAG_PLAIN: u8 = 2;

/// Mutable state reachable from inside the Unicorn hooks.
pub struct MachineState {
    heap_cursor: u64,
    heap_high: u64,
    capturing: bool,
    /// Per heap byte (index = addr - HEAP_BASE): last plaintext value.
    plain: Vec<u8>,
    /// Per heap byte: FLAG_READ | FLAG_PLAIN.
    flags: Vec<u8>,
    /// Every block handed out since the last reset, as (address, size).
    /// The deserializer's own allocation sizes are exact, which is how
    /// `fields` knows an array's entry size instead of guessing it.
    allocs: Vec<(u64, u64)>,
}

impl MachineState {
    fn alloc(&mut self, size: u64) -> Option<u64> {
        let ptr = (self.heap_cursor + 15) & !15;
        let end = ptr + size.max(1);
        if end > HEAP_BASE + HEAP_SIZE {
            return None;
        }
        self.heap_cursor = end;
        self.heap_high = self.heap_high.max(end);
        self.allocs.push((ptr, size));
        Some(ptr)
    }

    fn touch(&mut self, addr: u64, size: usize) -> Option<std::ops::Range<usize>> {
        if !(HEAP_BASE..HEAP_BASE + HEAP_SIZE).contains(&addr) {
            return None;
        }
        let start = (addr - HEAP_BASE) as usize;
        let end = start + size;
        if self.flags.len() < end {
            let n = end.next_power_of_two().max(0x1_0000);
            self.flags.resize(n, 0);
            self.plain.resize(n, 0);
        }
        Some(start..end)
    }

    fn on_read(&mut self, addr: u64, size: usize) {
        if let Some(r) = self.touch(addr, size) {
            for f in &mut self.flags[r] {
                *f |= FLAG_READ;
            }
        }
    }

    fn on_write(&mut self, addr: u64, size: usize, value: i64) {
        let Some(r) = self.touch(addr, size) else {
            return;
        };
        let obfuscation_pass = size == 1 && self.flags[r.start] & FLAG_READ != 0;
        for (k, i) in r.enumerate() {
            if !obfuscation_pass {
                self.plain[i] = if k < 8 { (value >> (8 * k)) as u8 } else { 0 };
                self.flags[i] |= FLAG_PLAIN;
            }
            self.flags[i] &= !FLAG_READ;
        }
    }
}

/// Outcome of running one function to completion (or not).
#[derive(Debug, Clone, Copy)]
pub struct CallOutcome {
    pub rax: u64,
    /// The function returned to the sentinel return address.
    pub returned: bool,
    /// Where execution stopped when it did not return.
    pub pc: u64,
}

pub struct Machine {
    uc: Unicorn<'static, MachineState>,
    image_base: u64,
    image_end: u64,
}

impl Machine {
    pub fn new(archive: &PatchArchive) -> Result<Self> {
        let state = MachineState {
            heap_cursor: HEAP_BASE,
            heap_high: HEAP_BASE,
            allocs: Vec::new(),
            capturing: false,
            plain: Vec::new(),
            flags: Vec::new(),
        };
        let mut uc = Unicorn::new_with_data(Arch::X86, Mode::MODE_64, state)
            .map_err(|e| emu_err("create VM", e))?;
        // Unicorn reserves a 1 GiB translation buffer per VM by default,
        // which dominates once there is one VM per worker thread. The code
        // a deserializer touches fits easily in 64 MiB. Must be set before
        // the VM's first real use; `uc_ctl` stores it for lazy init.
        let tcg_buffer_write = 13 | (1 << 26) | (1 << 30); // UC_CTL_WRITE(TCG_BUFFER_SIZE, 1)
        unsafe {
            unicorn_engine::ffi::uc_ctl(uc.get_handle(), tcg_buffer_write, TCG_BUFFER_SIZE);
        }
        let base = archive.image_base;
        let image_end = base + align(archive.image_size);

        uc.mem_map(base, align(archive.image_size) as usize, Permission::ALL)
            .map_err(|e| emu_err("map image", e))?;
        for s in &archive.sections {
            let n = s.raw.len().min(s.size as usize);
            uc.mem_write(base + s.rva, &s.raw[..n])
                .map_err(|e| emu_err(&format!("write {}", s.name), e))?;
        }
        for (addr, size) in [
            (STACK_BASE, STACK_SIZE),
            (HEAP_BASE, HEAP_SIZE),
            (STOP_ADDR, PAGE),
            (TEB_BASE, 4 * PAGE),
            (STUB_BASE, PAGE),
        ] {
            uc.mem_map(addr, size as usize, Permission::READ | Permission::WRITE | Permission::EXEC)
                .map_err(|e| emu_err("map runtime region", e))?;
        }
        uc.mem_write(STOP_ADDR, &[0xF4; PAGE as usize])
            .map_err(|e| emu_err("write stop page", e))?;
        uc.mem_write(STUB_BASE, &RETURN_ZERO)
            .map_err(|e| emu_err("write stub", e))?;

        for slot in &archive.import_slots {
            uc.mem_write(base + slot, &STUB_BASE.to_le_bytes())
                .map_err(|e| emu_err("patch import slot", e))?;
        }
        for rva in &archive.free_rvas {
            uc.mem_write(base + rva, &[0xC3])
                .map_err(|e| emu_err("patch free", e))?;
        }

        // TEB at gs: self pointer, TLS array -> one TLS block for every index.
        let tls_block = TEB_BASE + 2 * PAGE;
        let template = archive
            .read(archive.tls_template_rva, archive.tls_size)
            .ok_or_else(|| RoflError::Emulator("TLS template outside the image".into()))?;
        uc.mem_write(tls_block, &template)
            .map_err(|e| emu_err("write TLS block", e))?;
        let array: Vec<u8> = std::iter::repeat(tls_block.to_le_bytes())
            .take(64)
            .flatten()
            .collect();
        uc.mem_write(TEB_BASE + PAGE, &array)
            .map_err(|e| emu_err("write TLS array", e))?;
        uc.mem_write(TEB_BASE + 0x30, &TEB_BASE.to_le_bytes())
            .map_err(|e| emu_err("write TEB", e))?;
        uc.mem_write(TEB_BASE + 0x58, &(TEB_BASE + PAGE).to_le_bytes())
            .map_err(|e| emu_err("write TEB", e))?;
        uc.reg_write(RegisterX86::GS_BASE, TEB_BASE)
            .map_err(|e| emu_err("set gs", e))?;

        if let Some(reg) = archive.team_registry {
            let obj = STUB_BASE + 0x100;
            uc.mem_write(obj + 8, &reg.count.to_le_bytes())
                .map_err(|e| emu_err("write team registry", e))?;
            uc.mem_write(obj + 0x70, &[1])
                .map_err(|e| emu_err("write team registry", e))?;
            uc.mem_write(base + reg.global_rva, &obj.to_le_bytes())
                .map_err(|e| emu_err("install team registry", e))?;
        }

        // Operator new: `ret` plus a hook that does the allocation.
        let alloc_va = base + archive.alloc_rva;
        uc.mem_write(alloc_va, &[0xC3])
            .map_err(|e| emu_err("patch alloc", e))?;
        uc.add_code_hook(alloc_va, alloc_va, |uc, _, _| {
            let size = uc.reg_read(RegisterX86::RCX).unwrap_or(0);
            let ptr = uc.get_data_mut().alloc(size).unwrap_or(0);
            let _ = uc.reg_write(RegisterX86::RAX, ptr);
        })
        .map_err(|e| emu_err("hook alloc", e))?;

        if let Some(rva) = archive.cfg_dispatch_rva {
            let va = base + rva;
            uc.add_code_hook(va, va, move |uc, _, _| {
                let target = uc.reg_read(RegisterX86::RAX).unwrap_or(0);
                if !is_mapped(target, base, image_end) {
                    let _ = uc.reg_write(RegisterX86::RAX, STUB_BASE);
                }
            })
            .map_err(|e| emu_err("hook CFG dispatch", e))?;
        }

        let heap_end = HEAP_BASE + HEAP_SIZE - 1;
        uc.add_mem_hook(HookType::MEM_READ, HEAP_BASE, heap_end, |uc, _: MemType, addr, size, _| {
            let st = uc.get_data_mut();
            if st.capturing {
                st.on_read(addr, size);
            }
            true
        })
        .map_err(|e| emu_err("hook reads", e))?;
        uc.add_mem_hook(HookType::MEM_WRITE, HEAP_BASE, heap_end, |uc, _: MemType, addr, size, value| {
            let st = uc.get_data_mut();
            if st.capturing {
                st.on_write(addr, size, value);
            }
            true
        })
        .map_err(|e| emu_err("hook writes", e))?;

        Ok(Machine {
            uc,
            image_base: base,
            image_end,
        })
    }

    pub fn image_base(&self) -> u64 {
        self.image_base
    }

    /// Bump-allocate zeroed memory on the emulated heap.
    pub fn alloc(&mut self, size: u64) -> Result<u64> {
        self.uc
            .get_data_mut()
            .alloc(size)
            .ok_or_else(|| RoflError::Emulator("emulated heap exhausted".into()))
    }

    /// Zero everything allocated since the last reset and rewind the heap,
    /// so no bytes from one decode leak into the next.
    pub fn reset_heap(&mut self) -> Result<()> {
        let st = self.uc.get_data_mut();
        let used = (st.heap_high - HEAP_BASE) as usize;
        st.heap_cursor = HEAP_BASE;
        st.heap_high = HEAP_BASE;
        st.allocs.clear();
        let n = used.min(st.flags.len());
        st.flags[..n].fill(0);
        st.plain[..n].fill(0);
        if used > 0 {
            self.uc
                .mem_write(HEAP_BASE, &vec![0u8; used])
                .map_err(|e| emu_err("clear heap", e))?;
        }
        Ok(())
    }

    pub fn heap_cursor(&self) -> u64 {
        self.uc.get_data().heap_cursor
    }

    /// Every heap block handed out since the last reset, (address, size).
    pub fn allocations(&self) -> &[(u64, u64)] {
        &self.uc.get_data().allocs
    }

    pub fn set_capture(&mut self, on: bool) {
        self.uc.get_data_mut().capturing = on;
    }

    /// Heap bytes `[HEAP_BASE, end)` with every recorded plaintext value
    /// substituted for the in-memory (obfuscated) byte.
    pub fn plaintext_heap(&self, end: u64) -> Result<(Vec<u8>, Vec<bool>)> {
        let len = (end.saturating_sub(HEAP_BASE)) as usize;
        let mut bytes = vec![0u8; len];
        self.uc
            .mem_read(HEAP_BASE, &mut bytes)
            .map_err(|e| emu_err("read heap", e))?;
        let st = self.uc.get_data();
        let mut written = vec![false; len];
        for i in 0..len.min(st.flags.len()) {
            if st.flags[i] & FLAG_PLAIN != 0 {
                bytes[i] = st.plain[i];
                written[i] = true;
            }
        }
        Ok((bytes, written))
    }

    pub fn write(&mut self, addr: u64, bytes: &[u8]) -> Result<()> {
        self.uc
            .mem_write(addr, bytes)
            .map_err(|e| emu_err("write", e))
    }

    pub fn read_u64(&self, addr: u64) -> Result<u64> {
        let mut b = [0u8; 8];
        self.uc
            .mem_read(addr, &mut b)
            .map_err(|e| emu_err("read u64", e))?;
        Ok(u64::from_le_bytes(b))
    }

    pub fn read_u32(&self, addr: u64) -> Result<u32> {
        let mut b = [0u8; 4];
        self.uc
            .mem_read(addr, &mut b)
            .map_err(|e| emu_err("read u32", e))?;
        Ok(u32::from_le_bytes(b))
    }

    /// Run the function at virtual address `va` with up to four integer
    /// arguments (Windows x64: RCX, RDX, R8, R9) until it returns.
    pub fn call(&mut self, va: u64, args: &[u64]) -> Result<CallOutcome> {
        // At function entry RSP is 8 mod 16 (the caller's return address).
        let rsp = STACK_BASE + STACK_SIZE - 0x2008;
        self.uc
            .mem_write(rsp, &STOP_ADDR.to_le_bytes())
            .map_err(|e| emu_err("push return address", e))?;
        let regs = [RegisterX86::RCX, RegisterX86::RDX, RegisterX86::R8, RegisterX86::R9];
        for (reg, value) in regs.iter().zip(args) {
            self.uc
                .reg_write(*reg, *value)
                .map_err(|e| emu_err("set argument", e))?;
        }
        self.uc
            .reg_write(RegisterX86::RSP, rsp)
            .map_err(|e| emu_err("set rsp", e))?;
        let _ = self.uc.emu_start(va, STOP_ADDR, 0, INSTRUCTION_LIMIT);
        let pc = self.uc.pc_read().unwrap_or(0);
        Ok(CallOutcome {
            rax: self.uc.reg_read(RegisterX86::RAX).unwrap_or(0),
            returned: pc == STOP_ADDR,
            pc,
        })
    }

    /// `call` for a function given by image RVA.
    pub fn call_rva(&mut self, rva: u64, args: &[u64]) -> Result<CallOutcome> {
        self.call(self.image_base + rva, args)
    }

    pub fn is_mapped(&self, addr: u64) -> bool {
        is_mapped(addr, self.image_base, self.image_end)
    }
}

fn is_mapped(addr: u64, image_base: u64, image_end: u64) -> bool {
    (image_base..image_end).contains(&addr)
        || (HEAP_BASE..HEAP_BASE + HEAP_SIZE).contains(&addr)
        || (STACK_BASE..STACK_BASE + STACK_SIZE).contains(&addr)
        || (STUB_BASE..STUB_BASE + PAGE).contains(&addr)
}

fn align(size: u64) -> u64 {
    (size + PAGE - 1) & !(PAGE - 1)
}

fn emu_err(ctx: &str, err: unicorn_engine::unicorn_const::uc_error) -> RoflError {
    RoflError::Emulator(format!("{ctx}: {err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> MachineState {
        MachineState {
            heap_cursor: HEAP_BASE,
            heap_high: HEAP_BASE,
            allocs: Vec::new(),
            capturing: true,
            plain: Vec::new(),
            flags: Vec::new(),
        }
    }

    #[test]
    fn wide_store_then_byte_obfuscation_keeps_plaintext() {
        let mut st = state();
        let a = HEAP_BASE + 0x20;
        st.on_write(a, 4, 0x4000_00AEu32 as i64);
        for k in 0..4 {
            st.on_read(a + k, 1);
            st.on_write(a + k, 1, 0x5A);
        }
        let i = (a - HEAP_BASE) as usize;
        assert_eq!(&st.plain[i..i + 4], &0x4000_00AEu32.to_le_bytes());
    }

    #[test]
    fn dword_accumulation_is_a_value_update() {
        // Varint readers OR 7-bit groups straight into the destination.
        let mut st = state();
        let a = HEAP_BASE + 0x40;
        st.on_write(a, 4, 0);
        st.on_read(a, 4);
        st.on_write(a, 4, 0x7F);
        st.on_read(a, 4);
        st.on_write(a, 4, 0x3F7F);
        let i = (a - HEAP_BASE) as usize;
        assert_eq!(&st.plain[i..i + 4], &0x3F7Fu32.to_le_bytes());
    }

    #[test]
    fn byte_field_written_once_is_plaintext() {
        let mut st = state();
        let a = HEAP_BASE + 0x10;
        st.on_write(a, 1, 5);
        let i = (a - HEAP_BASE) as usize;
        assert_eq!(st.plain[i], 5);
        assert_ne!(st.flags[i] & FLAG_PLAIN, 0);
    }

    #[test]
    fn writes_outside_the_heap_are_ignored() {
        let mut st = state();
        st.on_write(STACK_BASE, 8, -1);
        assert!(st.flags.is_empty());
    }
}
