import AppKit
import Darwin
import SwiftUI

@main
struct GyroflowFinalCutApplication: App {
    init() {
        if CommandLine.arguments.contains("--localization-report-and-quit") {
            let keys = ["app.title", "app.subtitle", "app.action.about",
                        "app.action.choose_fcpxml", "app.action.retry_open"]
            let report: [String: Any] = [
                "bundlePath": Bundle.main.bundlePath,
                "preferredLanguages": Locale.preferredLanguages,
                "preferredLocalizations": Bundle.main.preferredLocalizations,
                "localizations": Bundle.main.localizations,
                "strings": Dictionary(uniqueKeysWithValues: keys.map {
                    ($0, FinalCutStrings.text($0))
                }),
            ]
            do {
                let data = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
                FileHandle.standardOutput.write(data)
                print("")
                exit(0)
            } catch {
                fputs("Unable to encode localization report\n", stderr)
                exit(1)
            }
        }
        let installTemplate = CommandLine.arguments.contains(
            "--install-template-and-quit"
        )
        let removeTemplate = CommandLine.arguments.contains(
            "--remove-template-and-quit"
        )
        let preflightTemplate = CommandLine.arguments.contains(
            "--preflight-template-and-quit"
        )
        guard installTemplate || removeTemplate || preflightTemplate else {
            return
        }
        do {
            let installer = try TemplateInstaller.production()
            if preflightTemplate {
                try installer.preflightInstallOrRepair()
                print("Final Cut template preflight passed")
                exit(TemplateInstallExitCode.success)
            }
            if removeTemplate {
                try installer.remove()
                print("Final Cut template removed")
                exit(TemplateInstallExitCode.success)
            }
            let status = try installer.installOrRepair()
            guard status.state == .installed else {
                fputs("Final Cut template verification failed after install\n", stderr)
                exit(TemplateInstallExitCode.verificationFailed)
            }
            print("Final Cut template installed: \(status.message)")
            exit(TemplateInstallExitCode.success)
        } catch {
            fputs("Final Cut template install failed: \(error.localizedDescription)\n", stderr)
            exit(TemplateInstallExitCode.installFailed)
        }
    }

    var body: some Scene {
        WindowGroup(FinalCutStrings.text("app.title")) {
            BatchProcessView()
        }
        .defaultSize(width: 580, height: 240)
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button(FinalCutStrings.text("app.action.about")) {
                    NSApplication.shared.orderFrontStandardAboutPanel(options: [
                        .applicationName: FinalCutStrings.text("app.title"),
                        .credits: NSAttributedString(string: FinalCutStrings.text("app.about.description")),
                    ])
                }
            }
        }
    }
}
