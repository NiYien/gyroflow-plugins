import Foundation

enum FinalCutStrings {
    static func text(
        _ key: String,
        fallback: String? = nil,
        bundle: Bundle = .main
    ) -> String {
        bundle.localizedString(forKey: key, value: fallback ?? key, table: nil)
    }

    static func format(
        _ key: String,
        _ arguments: CVarArg...,
        bundle: Bundle = .main
    ) -> String {
        String(
            format: text(key, bundle: bundle),
            locale: Locale.current,
            arguments: arguments
        )
    }

    static func plural(
        _ key: String,
        count: Int,
        bundle: Bundle = .main
    ) -> String {
        String.localizedStringWithFormat(text(key, bundle: bundle), count)
    }
}
