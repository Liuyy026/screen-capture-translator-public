import Foundation
import Darwin

/// One application-owned Rust process. Swift only transports requests/events;
/// page queues, cache writes, model lifetime and display generation live in Rust.
@MainActor final class NativeCoreService {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    private struct Pending {
        let continuation: CheckedContinuation<Data, Error>
        let progress: (([String: Any]) -> Void)?
    }
    private let executable: URL
    private let arguments: [String]
    private let logURL: URL
    private var child: Process?
    private var input: FileHandle?
    private var log: FileHandle?
    private var generation = UUID()
    private var pending: [String: Pending] = [:]
    private let writer = DispatchQueue(label: "KomaRust.requests")
    private var closed = false

    init(root: URL, cache: URL, assets: URL, seed: URL? = nil) {
        executable = root.appendingPathComponent("core/target/release/koma-core")
        arguments = ["serve", "--cache-root", cache.path, "--assets-root", assets.path,
                     "--python", root.appendingPathComponent("macos/.venv/bin/python").path,
                     "--worker", root.appendingPathComponent("core/python/model_worker.py").path,
                     "--weights", root.appendingPathComponent(".models").path,
                     "--ollama", root.appendingPathComponent("macos/.runtime/ollama/ollama").path]
            + (seed.map { ["--seed-root", $0.path] } ?? [])
        logURL = assets.deletingLastPathComponent().appendingPathComponent("native-core.log")
    }

    // Also used by the transport harness with a private scripted peer.
    init(executable: URL, arguments: [String], logURL: URL) {
        self.executable = executable; self.arguments = arguments; self.logURL = logURL
    }

    private func start() throws {
        guard !closed else { throw Failure(message: "处理服务已关闭，请重新打开应用。") }
        if child?.isRunning == true { return }
        guard pending.isEmpty else { throw Failure(message: "处理服务正在退出，请稍后重试。") }
        let process = Process(), incoming = Pipe(), outgoing = Pipe()
        process.executableURL = executable; process.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "KOMA_TRANSLATION_API_KEY")
        process.environment = environment
        process.standardInput = incoming; process.standardOutput = outgoing
        try FileManager.default.createDirectory(at: logURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !FileManager.default.fileExists(atPath: logURL.path) {
            FileManager.default.createFile(atPath: logURL.path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        let log = try FileHandle(forWritingTo: logURL)
        try log.seekToEnd()
        process.standardError = log
        let epoch = UUID(); generation = epoch
        do { try process.run() }
        catch { try? log.close(); throw Failure(message: "无法启动 Rust 处理服务，请检查候选版本的构建。") }
        child = process; input = incoming.fileHandleForWriting; self.log = log
        let stream = outgoing.fileHandleForReading
        // Drain continuously on a background queue. Reading only after process
        // termination would deadlock on a full pipe containing image/contour JSON.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            var buffer = Data()
            var chunk = [UInt8](repeating: 0, count: 65536)
            do {
                while true {
                    // FileHandle.read(upToCount:) can wait for the requested
                    // count on a pipe; POSIX read returns each available event.
                    let count = Darwin.read(stream.fileDescriptor, &chunk, chunk.count)
                    if count < 0 && errno == EINTR { continue }
                    if count <= 0 { break }
                    buffer.append(contentsOf: chunk.prefix(count))
                    while let newline = buffer.firstIndex(of: 10) {
                        let line = Data(buffer[..<newline])
                        buffer.removeSubrange(...newline)
                        guard line.count <= 128 * 1024 * 1024 else { throw Failure(message: "处理服务返回的数据过大。") }
                        DispatchQueue.main.async { [weak self] in self?.receive(line, epoch: epoch) }
                    }
                    guard buffer.count <= 128 * 1024 * 1024 else { throw Failure(message: "处理服务返回的数据过大。") }
                }
            } catch { /* Report a generic transport error, never raw payloads. */ }
            try? stream.close()
            DispatchQueue.main.async { [weak self] in self?.ended(epoch: epoch) }
        }
    }

    private func send(_ object: [String: Any]) throws {
        guard let input else { throw Failure(message: "处理服务未启动。") }
        var data = try JSONSerialization.data(withJSONObject: object)
        data.append(10)
        guard data.count <= 1024 * 1024 else { throw Failure(message: "请求过大，请减少一次处理的页数。") }
        let epoch = generation
        let payload = data
        writer.async { [weak self] in
            do { try input.write(contentsOf: payload) }
            catch { Task { @MainActor [weak self] in self?.ended(epoch: epoch) } }
        }
    }

    func request(_ operation: String, values: [String: Any] = [:],
                 progress: (([String: Any]) -> Void)? = nil) async throws -> Data {
        try Task.checkCancellation()
        try start()
        let id = UUID().uuidString
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                pending[id] = Pending(continuation: continuation, progress: progress)
                do {
                    var object = values; object["id"] = id; object["op"] = operation
                    try send(object)
                } catch {
                    pending.removeValue(forKey: id)?.continuation.resume(throwing: error)
                }
            }
        } onCancel: {
            Task { @MainActor [weak self] in
                guard let self, self.pending[id] != nil else { return }
                // Keep the continuation until Rust confirms cancellation and
                // has disposed of any interrupted model channel.
                try? self.send(["id": UUID().uuidString, "op": "cancel", "target": id])
            }
        }
    }

    private func receive(_ data: Data, epoch: UUID) {
        guard epoch == generation else { return }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = object["id"] as? String, let event = object["event"] as? String else {
            ended(epoch: epoch); return
        }
        guard let request = pending[id] else { return }
        switch event {
        case "result":
            pending.removeValue(forKey: id)
            do { request.continuation.resume(returning: try JSONSerialization.data(withJSONObject: object["result"] ?? NSNull(), options: [.fragmentsAllowed])) }
            catch { request.continuation.resume(throwing: Failure(message: "处理服务返回了无效结果。")) }
        case "error":
            pending.removeValue(forKey: id)
            request.continuation.resume(throwing: Failure(message: object["error"] as? String ?? "处理失败。"))
        case "cancelled":
            pending.removeValue(forKey: id)
            request.continuation.resume(throwing: CancellationError())
        default: request.progress?(object)
        }
    }

    private func ended(epoch: UUID) {
        guard epoch == generation else { return }
        generation = UUID()
        // EOF is the Rust shutdown signal; it cancels jobs and reaps its models.
        if let input { writer.async { try? input.close() } }
        input = nil; child = nil; try? log?.close(); log = nil
        let waiting = pending; pending.removeAll()
        for request in waiting.values {
            request.continuation.resume(throwing: Failure(message: "处理服务已退出；已保存的结果保留，可重新操作。"))
        }
    }

    func shutdown() {
        closed = true
        ended(epoch: generation)
    }
}
