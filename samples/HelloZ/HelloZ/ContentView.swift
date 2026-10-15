import SwiftUI

struct ContentView: View {
    @State private var counter = Counter()

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "hammer.fill")
                .imageScale(.large)
                .foregroundStyle(.tint)
            Text("Hello from zcode!")
                .font(.title)
            Text("Tapped \(counter.count) times")
            Button("Tap me") {
                counter.increment()
                print("tap \(counter.count)")
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
        .onAppear {
            print("HelloZ launched")
        }
    }
}

#Preview {
    ContentView()
}
