import XCTest
@testable import ZKit

final class GreeterXCTests: XCTestCase {
    func testPlainGreeting() {
        XCTAssertEqual(Greeter(name: "C").greeting(), "Hello, C.")
    }
}
