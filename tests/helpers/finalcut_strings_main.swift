import Foundation

@main
struct FinalCutStringsProbe {
    static func main() {
        guard CommandLine.arguments.count == 2,
              let bundle = Bundle(path: CommandLine.arguments[1])
        else {
            exit(64)
        }
        print(FinalCutStrings.text(
            "probe.key",
            fallback: "main fallback",
            bundle: bundle
        ))
    }
}
