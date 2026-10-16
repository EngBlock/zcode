import XCTest
@testable import HelloZ

final class LegacyTests: XCTestCase {
    func testStartsAtZero() {
        XCTAssertEqual(Counter().count, 0)
    }

    func testIncrementTwice() {
        var counter = Counter()
        counter.increment()
        counter.increment()
        XCTAssertEqual(counter.count, 2)
    }
}
