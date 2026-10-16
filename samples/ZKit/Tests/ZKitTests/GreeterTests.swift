import Foundation
import Testing
@testable import ZKit

@Test func plainGreeting() {
    #expect(Greeter(name: "A").greeting() == "Hello, A.")
}

@Test func excitedGreeting() {
    #expect(Greeter(name: "B").greeting(excited: true) == "Hello, B!")
}

@Test func schemeEnvironment() {
    // Edit Scheme's Test environment reaches the tests (scripts/e2e.py reads this).
    if let value = ProcessInfo.processInfo.environment["ZKIT_TEST_ENV"] {
        print("ZKIT_TEST_ENV=\(value)")
    }
}
