import Foundation
@testable import JCodeKit

runProtocolTests()
runClientTests()

let total = passed + failed + passed2 + failed2
let totalFailed = failed + failed2
print("\n" + String(repeating: "=", count: 40))
if totalFailed == 0 {
    print("TOTAL: \(total) assertions passed ✅")
} else {
    print("TOTAL: \(total - totalFailed) passed, \(totalFailed) FAILED ❌")
    exit(1)
}
