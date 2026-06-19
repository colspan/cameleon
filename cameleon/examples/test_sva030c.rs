use cameleon::u3v::enumerate_cameras;

fn main() {
    println!("Searching for USB3 Vision cameras...");

    let mut cameras = match enumerate_cameras() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to enumerate cameras: {}", e);
            println!();
            println!("Note: On Linux, USB access may require root or a udev rule.");
            println!("     Try: sudo cargo run --example test_sva030c --features libusb");
            println!("     Or add: SUBSYSTEM==\"usb\", ATTR{{idVendor}}==\"2bdf\", ATTR{{idProduct}}==\"0001\", MODE=\"0666\"");
            return;
        }
    };

    if cameras.is_empty() {
        println!("No camera found.");
        return;
    }

    println!("Found {} camera(s):", cameras.len());
    for (i, cam) in cameras.iter().enumerate() {
        let info = cam.info();
        println!(
            "  [{}] Vendor: {}, Model: {}, Serial: {}",
            i, info.vendor_name, info.model_name, info.serial_number
        );
    }

    println!("\nTesting camera...");
    let mut camera = cameras.pop().unwrap();

    println!("Opening camera...");
    if let Err(e) = camera.open() {
        eprintln!("Failed to open camera: {}", e);
        return;
    }
    println!("Camera opened successfully.");

    println!("Loading GenApi context...");
    if let Err(e) = camera.load_context() {
        eprintln!("Failed to load context: {}", e);
        return;
    }
    println!("GenApi context loaded.");

    println!("Starting stream...");
    let payload_rx = match camera.start_streaming(3) {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("Failed to start streaming: {}", e);
            return;
        }
    };
    println!("Streaming started.");

    println!("\nReceiving 10 frames...");
    for i in 0..10 {
        let payload = match payload_rx.recv_blocking() {
            Ok(p) => p,
            Err(e) => {
                println!("  Frame {}: payload receive error: {}", i + 1, e);
                continue;
            }
        };
        let id = payload.id();
        let ts = payload.timestamp();
        println!(
            "  Frame {}: block_id = {:?}, timestamp = {:?}",
            i + 1, id, ts
        );
        if let Some(info) = payload.image_info() {
            println!(
                "    Image: width = {}, height = {}, format = {:?}, size = {} bytes",
                info.width, info.height, info.pixel_format, info.image_size
            );
        }
        payload_rx.send_back(payload);
    }

    camera.close().ok();
    println!("\nCamera closed. Done.");
}
