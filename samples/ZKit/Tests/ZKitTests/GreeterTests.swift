import Testing
@testable import ZKit

@Test func plainGreeting() {
    #expect(Greeter(name: "A").greeting() == "Hello, A.")
}

@Test func excitedGreeting() {
    #expect(Greeter(name: "B").greeting(excited: true) == "Hello, B!")
}
