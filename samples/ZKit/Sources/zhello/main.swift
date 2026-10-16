import Foundation
import ZKit

// Edit Scheme's environment variables and arguments reach the program.
let name = ProcessInfo.processInfo.environment["ZHELLO_NAME"] ?? "zcode"
let greeter = Greeter(name: name)
print(greeter.greeting(excited: true))
let arguments = CommandLine.arguments.dropFirst()
if !arguments.isEmpty {
    print("arguments: \(arguments.joined(separator: " "))")
}
for i in 1...3 {
    print("tick \(i)")
}
