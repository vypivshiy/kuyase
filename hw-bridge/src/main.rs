mod audio;
mod devices;
mod http;
mod input;
mod ipc;
mod leds;
mod proto;
mod state;
mod tts;

use state::{BoardProfile, BridgeState};
use std::sync::Arc;

fn main() {
    audio::ensure_alsa_compat_lib();
    let profile = BoardProfile::detect();

    // Create /dev/input/event0..2 via /dev/uinput in exact order
    let tof_uinput = input::create_uinput_device(
        "STM VL53L0 proximity sensor",
        &[0, 3], // EV_SYN, EV_ABS
        &[],
        &[(0x12, 8191), (0x19, 255)],
    );
    let jack_uinput = input::create_uinput_device(
        "AML-AUGESOUND Headphones",
        &[0, 5], // EV_SYN, EV_SW
        &[2, 14],
        &[],
    );
    let key_uinput = input::create_uinput_device(
        "adc_keypad",
        &[0, 1], // EV_SYN, EV_KEY
        &[113, 114, 115, 582],
        &[],
    );

    let state = Arc::new(BridgeState::new(
        profile,
        tof_uinput,
        jack_uinput,
        key_uinput,
    ));

    state.log(format!(
        "Initialized Virtual Speaker Twin for {} (ring={} [{}], aux={} [{}], display={})",
        state.board_name,
        state.ring_leds,
        state.peripherals.ring.driver_name(),
        state.peripherals.aux_light.zone_count(),
        state.peripherals.aux_light.aux_kind(),
        state.peripherals.front_display.display_kind(),
    ));

    leds::spawn_native_led_animator(state.clone());
    ipc::spawn_yio_audio_server(state.clone());
    ipc::spawn_aliced_ipc_server(state.clone());
    ipc::spawn_iohub_services_ipc_server(state.clone());
    audio::spawn_alsa_loopback_capture(state.clone());
    input::spawn_serial_console_watcher();

    http::run_server(state);
}
