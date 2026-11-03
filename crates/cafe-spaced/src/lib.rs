use cafe_space_ipc::BuildFlavor;

pub fn run(flavor: BuildFlavor) {
    println!(
        "{} skeleton; daemon runtime is not implemented yet",
        flavor.daemon_name()
    );
}
