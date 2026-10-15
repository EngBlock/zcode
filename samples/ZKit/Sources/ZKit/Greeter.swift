/// Builds greetings.
public struct Greeter {
    public let name: String

    public init(name: String) {
        self.name = name
    }

    public func greeting(excited: Bool = false) -> String {
        "Hello, \(name)" + (excited ? "!" : ".")
    }
}
