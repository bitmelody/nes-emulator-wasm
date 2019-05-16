cargo run --release tests/ppu/vbl_nmi.nes
cargo run --release tests/ppu/oam_stress.nes
cargo run --release tests/ppu/palette.nes
cargo run --release tests/ppu/vbl_nmi_timing/1.frame_basics.nes
cargo run --release tests/ppu/vbl_nmi_timing/2.vbl_timing.nes
cargo run --release tests/ppu/vbl_nmi_timing/3.even_odd_frames.nes
cargo run --release tests/ppu/vbl_nmi_timing/5.nmi_suppression.nes
cargo run --release tests/ppu/vbl_nmi_timing/6.nmi_disable.nes
cargo run --release tests/ppu/vbl_nmi_timing/7.nmi_timing.nes
cargo run --release tests/ppu/open_bus.nes
cargo run --release tests/ppu/sprite_overflow.nes
cargo run --release tests/ppu/oamtest3.nes
cargo run --release tests/ppu/vbl_clear_time.nes
cargo run --release tests/ppu/nmi_sync_ntsc.nes
cargo run --release tests/ppu/scanline.nes
cargo run --release tests/ppu/tv.nes
cargo run --release tests/cpu/exec_space_apu.nes
cargo run --release tests/cpu/interrupts.nes
cargo run --release tests/cpu/flag_concurrency.nes
cargo run --release tests/cpu/instr_timing.nes
cargo run --release tests/cpu/instr_misc.nes
cargo run --release tests/ppu/sprdma_and_dmc_dma.nes
cargo run --release tests/ppu/sprdma_and_dmc_dma_512.nes
