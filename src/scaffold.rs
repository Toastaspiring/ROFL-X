//! Scaffold the catalog + test boilerplate for a new packet type.
//!
//! Three artefacts are produced, matching the Phase 4 discipline in
//! `docs/ROADMAP.md`:
//!
//!   1. A `tests/handler_<name>.rs` integration test stub. It loads any
//!      fixtures named `<name>_*.bin` under `tests/fixtures/` and asserts
//!      they are non-empty. Once the user wires up a real decoder, the
//!      assertions get replaced with field-level checks.
//!   2. A markdown row appended to `docs/PACKETS.md` under the chosen
//!      status section (`OBSERVED-ONLY` or `PARTIAL` typically).
//!   3. A short next-steps checklist printed to stderr.
//!
//! What is *not* produced: a `src/packet/handlers/<name>.rs` file. The
//! production handler depends on the StubEmulator integration pattern,
//! which differs per decoder (write-hook vs. final-value, slot count, etc.)
//! and is too project-specific to template usefully. The user writes that
//! file when they have a working RVA range from `trace-decoder`.

use std::path::{Path, PathBuf};

use crate::error::{Result, RoflError};

/// One scaffolding job.
pub struct Scaffold {
    /// Lower-snake-case packet name (e.g. `"create_hero"`).
    pub name: String,
    /// Patch tag the decoder was discovered on (e.g. `"15.5"`).
    pub patch: String,
    /// Numeric netid on that patch.
    pub netid: u16,
    /// Optional decoder RVA range, hex strings.
    pub decoder_rva: Option<(String, String)>,
    /// Status to record in PACKETS.md. Must be one of the four catalog
    /// statuses defined in that doc.
    pub status: Status,
    /// One-line summary of what the packet is for, free-form.
    pub summary: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Status {
    Documented,
    Partial,
    ObservedOnly,
    Unknown,
}

impl Status {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "documented" => Ok(Status::Documented),
            "partial" => Ok(Status::Partial),
            "observed-only" | "observed_only" | "observed" => Ok(Status::ObservedOnly),
            "unknown" => Ok(Status::Unknown),
            other => Err(RoflError::Io(std::io::Error::other(format!(
                "unknown status {other:?}; expected DOCUMENTED, PARTIAL, OBSERVED-ONLY, or UNKNOWN"
            )))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Documented => "DOCUMENTED",
            Status::Partial => "PARTIAL",
            Status::ObservedOnly => "OBSERVED-ONLY",
            Status::Unknown => "UNKNOWN",
        }
    }
}

/// Run the scaffold, writing files relative to `repo_root`.
pub fn run(repo_root: &Path, scaffold: &Scaffold) -> Result<Vec<PathBuf>> {
    if !is_snake_case(&scaffold.name) {
        return Err(RoflError::Io(std::io::Error::other(format!(
            "name {:?} must be lower_snake_case",
            scaffold.name
        ))));
    }

    let mut written = Vec::new();

    // 1. Test stub.
    let tests_dir = repo_root.join("tests");
    std::fs::create_dir_all(&tests_dir)?;
    let test_path = tests_dir.join(format!("handler_{}.rs", scaffold.name));
    if test_path.exists() {
        return Err(RoflError::Io(std::io::Error::other(format!(
            "{} already exists; refusing to overwrite",
            test_path.display()
        ))));
    }
    std::fs::write(&test_path, render_test(&scaffold.name))?;
    written.push(test_path);

    // 2. PACKETS.md append.
    let packets_md = repo_root.join("docs").join("PACKETS.md");
    if packets_md.exists() {
        let entry = render_packets_entry(scaffold);
        let mut existing = std::fs::read_to_string(&packets_md)?;
        if existing.contains(&format!("### `{}`", scaffold.name.to_uppercase())) {
            return Err(RoflError::Io(std::io::Error::other(format!(
                "PACKETS.md already has an entry for `{}` (uppercase header)",
                scaffold.name.to_uppercase()
            ))));
        }
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&entry);
        std::fs::write(&packets_md, existing)?;
        written.push(packets_md);
    }

    Ok(written)
}

fn render_test(name: &str) -> String {
    format!(
        "//! Scaffolded by `rofl-x new-handler`.\n\
         //!\n\
         //! Round-trip test stub for the `{name}` packet. Loads any fixture\n\
         //! files at `tests/fixtures/{name}_*.bin` (produced by\n\
         //! `rofl-x extract-fixture`) and asserts they're non-empty. Once a\n\
         //! real decoder is wired up under `src/`, replace the body with\n\
         //! field-level assertions on the decoded struct.\n\
         \n\
         use std::fs;\n\
         use std::path::PathBuf;\n\
         \n\
         fn fixtures_dir() -> PathBuf {{\n\
         \x20   PathBuf::from(env!(\"CARGO_MANIFEST_DIR\")).join(\"tests/fixtures\")\n\
         }}\n\
         \n\
         #[test]\n\
         fn fixtures_are_present_and_non_empty() {{\n\
         \x20   let dir = fixtures_dir();\n\
         \x20   if !dir.exists() {{\n\
         \x20       eprintln!(\"no tests/fixtures dir; skipping\");\n\
         \x20       return;\n\
         \x20   }}\n\
         \x20   let mut found = 0usize;\n\
         \x20   for entry in fs::read_dir(&dir).expect(\"read fixtures dir\") {{\n\
         \x20       let entry = entry.expect(\"dirent\");\n\
         \x20       let path = entry.path();\n\
         \x20       let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(\"\");\n\
         \x20       if !stem.starts_with(\"{name}_\") {{ continue; }}\n\
         \x20       if path.extension().and_then(|s| s.to_str()) != Some(\"bin\") {{ continue; }}\n\
         \x20       let bytes = fs::read(&path).expect(\"read fixture\");\n\
         \x20       assert!(!bytes.is_empty(), \"fixture {{}} is empty\", path.display());\n\
         \x20       found += 1;\n\
         \x20   }}\n\
         \x20   assert!(found > 0, \"no fixtures matching {name}_*.bin found in {{}}\", dir.display());\n\
         }}\n",
        name = name
    )
}

fn render_packets_entry(s: &Scaffold) -> String {
    let header = format!("### `{}`\n\n", s.name.to_uppercase());
    let status_line = format!("**Status:** {} on patch {}.\n", s.status.label(), s.patch);
    let netid_line = format!("**Patch-specific netid (patch {}):** {}\n", s.patch, s.netid);
    let rva_line = match &s.decoder_rva {
        Some((start, end)) => format!("**Decoder RVA:** `{start}..{end}`\n"),
        None => "**Decoder RVA:** unknown\n".to_string(),
    };
    let summary_line = if s.summary.is_empty() {
        String::new()
    } else {
        format!("\n**Summary:** {}\n", s.summary)
    };
    let provenance = "\n**Provenance:** scaffolded by `rofl-x new-handler`. Fixture(s) under `tests/fixtures/`; \
                      handler not yet wired into `src/` (production decoder requires the StubEmulator integration \
                      pattern from `src/emulator/packet.rs`).\n\n---\n";
    format!(
        "\n{}{}{}{}{}{}",
        header, status_line, netid_line, rva_line, summary_line, provenance
    )
}

fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_validator() {
        assert!(is_snake_case("create_hero"));
        assert!(is_snake_case("hero_die"));
        assert!(!is_snake_case("CreateHero"));
        assert!(!is_snake_case("create-hero"));
        assert!(!is_snake_case("_leading"));
        assert!(!is_snake_case("trailing_"));
        assert!(!is_snake_case(""));
    }

    #[test]
    fn renders_packets_entry_with_rva() {
        let s = Scaffold {
            name: "create_hero".to_string(),
            patch: "16.8".to_string(),
            netid: 0x123,
            decoder_rva: Some(("0xe40000".to_string(), "0xe40500".to_string())),
            status: Status::Partial,
            summary: "Champion init.".to_string(),
        };
        let out = render_packets_entry(&s);
        assert!(out.contains("### `CREATE_HERO`"));
        assert!(out.contains("PARTIAL"));
        assert!(out.contains("0xe40000..0xe40500"));
        assert!(out.contains("Champion init."));
    }
}
