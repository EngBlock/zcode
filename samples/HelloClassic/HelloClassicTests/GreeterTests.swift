import XCTest
@testable import HelloClassic

final class GreeterTests: XCTestCase {
    func testGreeting() {
        XCTAssertEqual(Greeter(name: "zcode").greeting, "Hello, zcode!")
    }
}
