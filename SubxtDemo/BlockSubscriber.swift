import Foundation

@MainActor
class BlockSubscriber: ObservableObject {
    @Published var status = "Not connected"
    @Published var blocks: [BlockInfo] = []
    @Published var isConnected = false
    @Published var useLightClient = true

    static let maxBlocks = 100
    private var task: Task<Void, Never>?
    private var cancel: CancellationToken?

    func start() {
        guard task == nil else { return }
        print("[swift] start()")
        status = "Starting..."
        blocks = []
        isConnected = true

        let token = CancellationToken()
        cancel = token

        let listener = Listener(subscriber: self)
        let mode: ConnectionMode = useLightClient
            ? .lightClient
            : .rpc(url: "wss://polkadot-asset-hub-rpc.polkadot.io:443")

        task = Task {
            do {
                try await subscribe(mode: mode, cancel: token, listener: listener)
            } catch {
                status = "Error: \(error.localizedDescription)"
            }
            print("[swift] subscribe() returned, cleaning up")
            isConnected = false
        }
    }

    func stop() {
        print("[swift] stop() called")
        cancel?.cancel()
        cancel = nil
        task = nil
        isConnected = false
        status = "Disconnected"
    }
}

private class Listener: BlockListener {
    private weak var subscriber: BlockSubscriber?

    init(subscriber: BlockSubscriber) {
        self.subscriber = subscriber
    }

    func onStatusChange(status: String) throws {
        let sub = subscriber
        Task { @MainActor in
            sub?.status = status
        }
    }

    func onNewBlock(block: BlockInfo) throws {
        let sub = subscriber
        Task { @MainActor in
            guard let sub else { return }
            sub.blocks.insert(block, at: 0)
            if sub.blocks.count > BlockSubscriber.maxBlocks {
                sub.blocks.removeLast(sub.blocks.count - BlockSubscriber.maxBlocks)
            }
            sub.status = "Block #\(block.number)"
        }
    }

    func onError(error: String) throws {
        let sub = subscriber
        Task { @MainActor in
            sub?.status = "Error: \(error)"
        }
    }
}
