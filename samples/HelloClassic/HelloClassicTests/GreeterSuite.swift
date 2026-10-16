import Testing
@testable import HelloClassic

@Suite struct GreeterSuite {
    @Test func greets() {
        #expect(Greeter(name: "a").greeting == "Hello, a!")
    }

    @Test("Greets the world") func world() {
        #expect(Greeter(name: "world").greeting.hasSuffix("world!"))
    }
}
