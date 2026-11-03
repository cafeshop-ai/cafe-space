use cafe_space_ipc::BuildFlavor;

pub fn run(flavor: BuildFlavor) {
    println!(
        "{} skeleton; CLI commands are not implemented yet",
        flavor.cli_name()
    );
}
