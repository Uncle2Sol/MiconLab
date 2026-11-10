use miconlab_ai::thumb_disasm::disassemble_thumb;

fn main() {
    // 测试各种32位Thumb-2指令
    let tests: Vec<(&str, &[u8])> = vec![
        ("ORR.W R3,R0,R1", &[0x40, 0xEA, 0x01, 0x03]),
        ("ORR.W (imm)", &[0x40, 0xF0, 0x01, 0x03]),
        ("STM.W R4,{R0-R2}", &[0x84, 0xE8, 0x07, 0x00]),
        ("LDM.W R4,{R0-R2}", &[0x94, 0xE8, 0x07, 0x00]),
        ("MOV.W (16bit imm)", &[0x40, 0xF2, 0x04, 0x60]),
        ("LDRD", &[0xD3, 0xE9, 0x01, 0x02]),
        ("UBFX", &[0xC4, 0xF3, 0x40, 0x00]),  // 0xF3C4 0x0040: UBFX R0, R4, #1, #1
        ("SUB.W (imm)", &[0xB4, 0xF1, 0x00, 0x4F]),
        ("STR.W R0,[R0,#0x14]", &[0xC0, 0xF8, 0x14, 0x00]),
        ("EOR.W (reg)", &[0x82, 0xEA, 0x01, 0x00]),
        ("ADD.W R1,R3,#0", &[0x03, 0xF1, 0x00, 0x01]),
        ("STR.W R0,[R4,#0x11]", &[0xC4, 0xF8, 0x11, 0x00]),
        ("ADD.W (imm2)", &[0x04, 0xF1, 0x20, 0x01]),
        ("BIC.W (imm mod)", &[0x20, 0xF4, 0x80, 0x20]),
        ("ORR.W (imm mod)", &[0x40, 0xF4, 0x70, 0x00]),
        ("STMDB.W R0!,{LR}", &[0x20, 0xE9, 0x00, 0x40]),
        ("LDR.W R0,[R4,#0x100]", &[0xD4, 0xF8, 0x00, 0x01]),
        ("MUL R0,R1,R2", &[0x01, 0xFB, 0x02, 0x00]),
        ("UDIV R0,R1,R2", &[0xB1, 0xFB, 0x02, 0xF0]),
        ("UMULL R0,R1,R2,R3", &[0xA2, 0xFB, 0x13, 0x00]),
    ];

    println!("=== 32-bit Thumb-2 instruction decode test ===");
    for (desc, data) in &tests {
        let inst = disassemble_thumb(data, 0x08000000, 0, 1);
        if inst.is_empty() {
            println!("✗ {}: no output", desc);
            continue;
        }
        let i = &inst[0];
        let status = if i.mnemonic == ".word" { "✗" } else { "✓" };
        println!("{} {}: {} {} {}",
            status, desc, i.mnemonic, i.operands,
            i.comment.as_ref().map(|c| c.as_str()).unwrap_or(""));
    }
}
