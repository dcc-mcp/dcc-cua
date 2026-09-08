#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    CurrentThread,
    MultiThread,
}

pub(crate) fn flavor(arguments: &[String]) -> Flavor {
    if arguments.first().is_some_and(|value| value == "mcp-server") {
        Flavor::CurrentThread
    } else {
        Flavor::MultiThread
    }
}

pub(crate) fn build(arguments: &[String]) -> std::io::Result<tokio::runtime::Runtime> {
    let mut builder = match flavor(arguments) {
        Flavor::CurrentThread => tokio::runtime::Builder::new_current_thread(),
        Flavor::MultiThread => tokio::runtime::Builder::new_multi_thread(),
    };
    builder.enable_all().build()
}
