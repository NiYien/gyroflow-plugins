import Foundation

struct SecurityScopedAccess: @unchecked Sendable {
    let start: (URL) -> Bool
    let stop: (URL) -> Void

    static let live = SecurityScopedAccess(
        start: { $0.startAccessingSecurityScopedResource() },
        stop: { $0.stopAccessingSecurityScopedResource() }
    )

    func withAccess<T>(to urls: [URL], _ body: () throws -> T) rethrows -> T {
        var startedURLs: [URL] = []
        for url in standardizedUniqueURLs(urls) {
            if start(url) {
                startedURLs.append(url)
            }
        }
        defer {
            for url in startedURLs.reversed() {
                stop(url)
            }
        }
        return try body()
    }

    private func standardizedUniqueURLs(_ urls: [URL]) -> [URL] {
        var uniqueURLs: [URL] = []
        var seenPaths = Set<String>()
        for url in urls {
            let standardized = url.standardizedFileURL
            if seenPaths.insert(standardized.path).inserted {
                uniqueURLs.append(standardized)
            }
        }
        return uniqueURLs
    }
}
