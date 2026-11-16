import Foundation
import Security

/// The signed-in jcode account (email + API key).
public struct AccountCredential: Codable, Equatable, Sendable {
    public var email: String
    public var apiKey: String
    public init(email: String, apiKey: String) {
        self.email = email
        self.apiKey = apiKey
    }
}

/// Keychain store for the single jcode account credential, with an
/// Application Support file fallback for unsigned simulator builds.
public struct AccountStore: Sendable {
    private static let service = "com.jcode.mobile.account"
    private static let account = "jcode-account"
    private let fileURL: URL?

    public init() {
        fileURL = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("jcode-account.json")
    }

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
        ]
    }

    public func load() -> AccountCredential? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        if SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess, let data = result as? Data {
            return try? JSONDecoder().decode(AccountCredential.self, from: data)
        }
        guard let fileURL, let data = try? Data(contentsOf: fileURL) else { return nil }
        return try? JSONDecoder().decode(AccountCredential.self, from: data)
    }

    public func save(_ credential: AccountCredential) {
        guard let data = try? JSONEncoder().encode(credential) else { return }
        SecItemDelete(query as CFDictionary)
        var q = query
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        if SecItemAdd(q as CFDictionary, nil) != errSecSuccess, let fileURL {
            try? FileManager.default.createDirectory(
                at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? data.write(to: fileURL, options: [.atomic, .completeFileProtection])
        }
    }

    public func clear() {
        SecItemDelete(query as CFDictionary)
        if let fileURL { try? FileManager.default.removeItem(at: fileURL) }
    }
}
