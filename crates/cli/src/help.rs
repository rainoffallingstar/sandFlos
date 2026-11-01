pub const USAGE: &str = "\
Usage: booxctl [OPTIONS] <COMMAND>

Commands:
  auth         Manage authentication and token storage
  push         Upload files and push them to the BOOX cloud
  queue        Inspect and manage the push queue
  devices      Inspect devices and local transfer addresses
  storage      Show cloud storage status
  reading      Show reading statistics
  calibre      Read a local Calibre library and push books
  zotero       Read a local Zotero library and push attachments
  diagnostics  Print an offline, redacted diagnostic report
  help         Print this help message

Options:
      --config-dir <PATH>  Use an isolated configuration directory
      --json               Emit one JSON envelope on stdout
      --no-input           Never prompt for interactive input
      --timeout <SECONDS>  Set the network timeout
      --verbose            Emit diagnostic details on stderr
  -h, --help               Print help
  -V, --version            Print version
";

pub const HELP: &str = "\
booxctl sends documents to BOOX devices from the command line.

Usage: booxctl [OPTIONS] <COMMAND>

Commands:
  auth         Manage authentication and token storage
  push         Upload files and push them to the BOOX cloud
  queue        Inspect and manage the push queue
  devices      Inspect devices and local transfer addresses
  storage      Show cloud storage status
  reading      Show reading statistics
  calibre      Read a local Calibre library and push books
  zotero       Read a local Zotero library and push attachments
  diagnostics  Print an offline, redacted diagnostic report
  help         Print this help message

Options:
      --config-dir <PATH>  Use an isolated configuration directory
      --json               Emit one JSON envelope on stdout
      --no-input           Never prompt for interactive input
      --timeout <SECONDS>  Set the network timeout
      --verbose            Emit diagnostic details on stderr
  -h, --help               Print help
  -V, --version            Print version
";
