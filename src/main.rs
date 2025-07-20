use std::path::*;
use std::process::*;
use std::time::*;
use std::fs::*;
use std::io::{Read, Seek, SeekFrom};

struct Args {
    pub asm_path: PathBuf,
    pub out_path: PathBuf,
    pub temp_path: PathBuf,
    pub as_path: PathBuf,
}

const USAGE: &'static str = "USAGE:
    hgecko <path/to/asm/folder> <path/to/output/codes.gct>
";

const ERROR_STR: &'static str = "\x1B[31mERROR:\x1B[0m";
const WARNING_STR: &'static str = "\x1B[33mWARNING:\x1B[0m";

fn parse_args() -> Args {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        print!("{}", USAGE);
        exit(1);
    }
    
    let devkitppc = match std::env::var_os("DEVKITPPC") {
        Some(d) => d,
        None => {
            eprintln!("{ERROR_STR} $DEVKITPPC environment variable is not set!
Please install devkitpro and the PPC/Gamecube package,
and ensure the DEVKITPPC environment variable is set.");
            exit(1);
        }
    };
    let as_path = Path::new(&devkitppc).join(Path::new("bin/powerpc-eabi-as"));
    
    let asm_path = Path::new(&args[1]).into();
    let out_path = Path::new(&args[2]).into();
    
    let args = Args {
        asm_path,
        out_path,
        temp_path: std::env::temp_dir(),
        as_path,
    };
    
    if !args.asm_path.try_exists().is_ok_and(|e| e) {
        eprintln!("{ERROR_STR} ASM path '{}' does not exist", args.asm_path.display());
        exit(1);
    }
    
    if !args.as_path.try_exists().is_ok_and(|e| e) {
        eprintln!("{ERROR_STR} GNU assembler path '{}' does not exist!", args.as_path.display());
        exit(1);
    }
    
    args
}

fn collect_asm(asm_paths: &mut Vec<PathBuf>, path: &Path) {
    let iter = match path.read_dir() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{WARNING_STR} Skipping directory '{}': {}", path.display(), e);
            return;
        }
    };
    
    for entry in iter {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        
        match entry.file_type() {
            Ok(f) if f.is_dir() => collect_asm(asm_paths, &path),
            Ok(f) if f.is_file() && path.extension() == Some("asm".as_ref()) => asm_paths.push(path),
            _ => {},
        }
    }
}

struct Code {
    pub addr: u32,
    pub code: Vec<u8>,
}

fn process_asm(args: &Args, paths: &[PathBuf]) -> Vec<Code> {
    // processes ~2 files per ms, bottleneck is spawning the child processes.
    
    // start all compilation jobs
    let mut jobs = start_compiling(args, paths);
    
    // while we wait for them to finish, read through the headers of all asm files for the injection address
    let mut codes = collect_headers(paths);
    
    // get the compiled asm from the compiled elfs and merge into codes.
    finish_compiling(&mut codes, &mut jobs, paths);
    
    codes
}

fn collect_headers(paths: &[PathBuf]) -> Vec<Code> {
    let mut codes = Vec::with_capacity(paths.len());
    let mut err = false;
    let mut buf = [0u8; 512];
    
    'file: for asm_path in paths.iter() {
        let mut f = match File::open(asm_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to open '{}': {}", asm_path.display(), e);
                err = true;
                continue 'file;
            }
        };
        
        let mut read = 0;
        let addr = 'find_addr: loop {
            if read == buf.len() {
                eprintln!("{ERROR_STR} File '{}' does not contain an injection address", asm_path.display());
                err = true;
                continue 'file;
            }
        
            match f.read(&mut buf[read..]) {
                Ok(0) => {
                    eprintln!("{ERROR_STR} File '{}' does not contain an injection address", asm_path.display());
                    err = true;
                    continue 'file;
                },
                Ok(n) => read += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("{ERROR_STR} Failed to read '{}': {}", asm_path.display(), e);
                    err = true;
                    continue 'file;
                }
            };
            
            'parse_addr: for w in buf[..read].windows(8) {
                if w[0] != b'8' { continue; }
                let mut addr = 0u32;
                for c in w.iter().copied() {
                    addr <<= 4;
                    match c {
                        b'0'..=b'9' => addr |= c as u32 - b'0' as u32,
                        b'a'..=b'f' => addr |= c as u32 - b'a' as u32 + 10,
                        b'A'..=b'F' => addr |= c as u32 - b'A' as u32 + 10,
                        _ => continue 'parse_addr
                    }
                }
                break 'find_addr addr;
            }
        };
        
        // ensure newline terminated
        // I hate that this is necessary
        {
            match f.seek(SeekFrom::End(-1)) {
                Ok(_) => {},
                Err(e) => {
                    eprintln!("{ERROR_STR} Failed to seek in '{}': {e}.", asm_path.display());
                    err = true;
                    continue 'file;
                }
            }
            let mut b = [0u8; 1];
            match f.read_exact(&mut b) {
                Ok(()) => {},
                Err(e) => {
                    eprintln!("{ERROR_STR} Failed to read '{}': {e}", asm_path.display());
                    err = true;
                    continue 'file;
                }
            }
            if b[0] != b'\n' {
                eprintln!("{ERROR_STR} ASM file '{}' is not newline terminated. ASM files MUST be newline terminated or they may be compiled incorrectly.", asm_path.display());
                err = true;
                continue 'file;
            }
        }
        
        codes.push(Code {
            addr,
            code: Vec::new(),
        })
    }
    
    if err { exit(1); }
    
    codes
}

fn hash_bytes(b: &[u8]) -> u32 {
    let mut h: u32 = 1234;
    for b in b {
        let b = *b as u32;
        h ^= b;
        h = h.wrapping_mul(0x5bd1e995);
        h ^= h >> 15;
    }
    h
}

struct AssembleJob {
    pub child: Child,
    pub out_path: PathBuf,
}

fn start_compiling(args: &Args, asm: &[PathBuf]) -> Vec<AssembleJob> {
    let mut jobs = Vec::with_capacity(asm.len());
    let mut err = false;
    
    for path in asm {
        let mut out_path = args.temp_path.to_path_buf();
        let mut hash = hash_bytes(path.as_os_str().as_encoded_bytes());
        let mut b = [0u8; 8];
        for i in 0..8 {
            let n = (hash & 0xf) as u8;
            b[i] = b'a' + n as u8;
            hash >>= 4;
        }
        out_path.push(unsafe { str::from_utf8_unchecked(&b) });
        
        let spawn = Command::new(&args.as_path)
            .arg("--warn")
            .arg("-mregnames")
            .arg("-mgekko")
            .arg("-mbig")
            .arg("-a32")
            .arg("-I")
            .arg(path.parent().unwrap())
            .arg("-o")
            .arg(&out_path)
            .arg(path)
            .spawn();
        
        match spawn {
            Ok(child) => jobs.push(AssembleJob { child, out_path }),
            Err(e) => {
                eprintln!("{ERROR_STR} Could not spawn compile process for '{}': {}", path.display(), e);
                err = true;
            },
        };
    }
    
    if err { exit(1); }
    jobs
}

fn finish_compiling(
    codes: &mut [Code],
    jobs: &mut [AssembleJob],
    paths: &[PathBuf],
) {
    let mut undef = Vec::new();
    let mut err = false;
    'file: for i in 0..codes.len() {
        if !jobs[i].child.wait().unwrap().success() {
            err = true;
            continue;
        }
        let code = &mut codes[i];
        let path = &paths[i];
        
        let mut elf_file = match File::open(&jobs[i].out_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to open compiled elf for '{}': {}", path.display(), e);
                err = true;
                continue 'file;
            }
        };
        let mut elf = match elf::ElfStream::<elf::endian::BigEndian, _>::open_stream(&mut elf_file) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to parse compiled elf for '{}': {}", path.display(), e);
                err = true;
                continue 'file;
            }
        };
        
        // check for undefined symbols
        let (symbol_table, string_table) = match elf.symbol_table() {
            Ok(Some(s)) => s,
            Ok(None) => {
                eprintln!("{ERROR_STR} Failed to extract string table and symbol table sections in compiled elf for '{}'", path.display());
                err = true;
                continue 'file;
            }
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to parse compiled elf for '{}': {}", path.display(), e);
                err = true;
                continue 'file;
            }
        };
        undef.clear();
        let mut symbol_iter = symbol_table.iter();
        symbol_iter.next(); // skip null entry
        for s in symbol_iter {
            if s.is_undefined() {
                undef.push(s);
            }
        }
        if !undef.is_empty() {
            undef.sort_by_key(|u| u.st_name);
            undef.dedup();
            for u in undef.iter() {
                let name = match string_table.get(u.st_name as usize) {
                    Ok("") | Err(_) => "(unnamed symbol)",
                    Ok(name) => name,
                };
                eprintln!("{WARNING_STR} Undefined symbol: {name}");
            }
            eprintln!("{WARNING_STR} {} undefined symbols in '{}'", undef.len(), path.display());
        }
        
        // Extract code
        let text_header = match elf.section_header_by_name(".text") {
            Ok(Some(f)) => *f,
            Ok(None) => {
                eprintln!("{ERROR_STR} Failed to extract .text section in compiled elf for '{}'", path.display());
                err = true;
                continue 'file;
            }
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to parse compiled elf for '{}': {}", path.display(), e);
                err = true;
                continue 'file;
            }
        };
        let text = match elf.section_data(&text_header) {
            Ok((b, None)) => b,
            Ok((_, Some(_))) => {
                eprintln!("{ERROR_STR} Cannot parse compressed sections for '{}'", path.display());
                err = true;
                continue 'file;
            }
            Err(e) => {
                eprintln!("{ERROR_STR} Failed to parse compiled elf for '{}': {}", path.display(), e);
                err = true;
                continue 'file;
            }
        };
        if text.is_empty() {
            eprintln!("{WARNING_STR} File '{}' has no ASM! Skipping...", path.display());
        }
        
        code.code = text.to_vec();
    }
    
    if err { exit(1); }
}

fn write_codes(args: &Args, codes: &[Code]) {
    let max_len = codes.iter().map(|c| c.code.len()).sum::<usize>() * 2;
    let mut data = Vec::with_capacity(max_len);
    
    data.extend_from_slice(&[0x00, 0xD0, 0xC0, 0xDE, 0x00, 0xD0, 0xC0, 0xDE]);
    
    for c in codes {
        if c.code.is_empty() { continue; }
    
        assert!(c.code.len() % 4 == 0);
        let mut addr = (c.addr - 0x80000000).to_be_bytes();
        if c.code.len() == 4 {
            addr[0] |= 0x04;
            data.extend_from_slice(&addr);
            data.extend_from_slice(c.code.as_slice());
        } else {
            addr[0] |= 0xC2;
            data.extend_from_slice(&addr);
            
            let code_words = c.code.len() as u32 / 4;
            let code_lines = if code_words & 1 == 0 {
                (code_words + 2) / 2
            } else {
                (code_words + 1) / 2
            };
            data.extend_from_slice(&code_lines.to_be_bytes());
            
            data.extend_from_slice(c.code.as_slice());
            
            if code_words & 1 == 0 {
                data.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
            }
            data.extend_from_slice(&[0x00; 4]);
        }
    }

    data.extend_from_slice(&[0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    match write(&args.out_path, &data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{ERROR_STR} Failed to create gct file '{}': {e}", args.out_path.display());
            exit(1);
        }
    };
}

fn main() {
    let t = Instant::now();
    let args = parse_args();
    let mut asm_paths = Vec::new();
    collect_asm(&mut asm_paths, &args.asm_path);
    let mut codes = process_asm(&args, &asm_paths);
    codes.sort_by_key(|c| c.addr);
    write_codes(&args, &codes);
    println!("processed {} files in {}ms", asm_paths.len(), t.elapsed().as_millis());
}
