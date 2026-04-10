import SwiftUI

struct ContentView: View {
    @StateObject private var subscriber = BlockSubscriber()

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                HStack {
                    Circle()
                        .fill(subscriber.isConnected ? .green : .orange)
                        .frame(width: 8, height: 8)
                    Text(subscriber.status)
                        .font(.caption.monospaced())
                        .lineLimit(1)
                    Spacer()
                }
                .padding(.horizontal)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial)

                List(subscriber.blocks, id: \.hash) { block in
                    VStack(alignment: .leading) {
                        Text("#\(block.number)")
                            .font(.headline.monospaced())
                        Text(block.hash)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                }
                .listStyle(.plain)
            }
            .navigationTitle("Asset Hub Blocks")
            .toolbar(content: {
                ToolbarItem(placement: .topBarLeading) {
                    Picker("Mode", selection: $subscriber.useLightClient) {
                        Text("RPC").tag(false)
                        Text("Light Client").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .fixedSize()
                    .disabled(subscriber.isConnected)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(subscriber.isConnected ? "Stop" : "Start") {
                        if subscriber.isConnected {
                            subscriber.stop()
                        } else {
                            subscriber.start()
                        }
                    }
                }
            })
        }
        .onAppear {
            subscriber.start()
        }
    }
}
