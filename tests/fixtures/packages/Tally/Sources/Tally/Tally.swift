/// Counts things. A fixture for zcode's package tests.
public struct Tally {
    public static let version = "0.0.0"

    public private(set) var count = 0

    public init() {}

    public mutating func add(_ n: Int = 1) {
        count += n
    }
}
