import Testing
@testable import HelloZ

struct CounterTests {
    @Test func incrementAddsOne() {
        var counter = Counter()
        counter.increment()
        #expect(counter.count == 1)
    }

    @Test func resetClearsCount() {
        var counter = Counter()
        counter.increment(by: 5)
        counter.reset()
        #expect(counter.count == 0)
    }
}
