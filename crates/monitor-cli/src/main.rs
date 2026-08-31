use mncs_host_collector::{Collector, PlatformCollector};

fn main() {
    let mut collector = PlatformCollector::new();

    println!("mncs-system-monitor (experimental bootstrap)");
    match collector.collect() {
        Ok(snapshot) => println!("collected {} process subjects", snapshot.process_count()),
        Err(error) => {
            println!("collection status: UNKNOWN ({error})");
            println!("no synthetic snapshot was emitted");
        }
    }
}
