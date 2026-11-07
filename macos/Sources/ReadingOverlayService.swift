import Foundation

private final class OverlayProcess: @unchecked Sendable {
    private let lock = NSLock()
    private var process: Process?
    private var cancelled = false

    func run(_ executable: URL, arguments: [String]) throws -> Data {
        let child = Process()
        child.executableURL = executable
        child.arguments = arguments
        let output = Pipe()
        child.standardOutput = output
        child.standardError = FileHandle.nullDevice
        lock.lock()
        if cancelled { lock.unlock(); throw CancellationError() }
        process = child
        do { try child.run() } catch { process = nil; lock.unlock(); throw error }
        lock.unlock()
        // Drain while the child runs, so large contour JSON cannot fill the pipe and deadlock.
        let data = output.fileHandleForReading.readDataToEndOfFile()
        child.waitUntilExit()
        lock.lock(); process = nil; let wasCancelled = cancelled; lock.unlock()
        if wasCancelled { throw CancellationError() }
        guard child.terminationStatus == 0 else { throw CocoaError(.fileReadUnknown) }
        return data
    }

    func cancel() {
        lock.lock(); defer { lock.unlock() }
        cancelled = true
        if process?.isRunning == true { process?.terminate() }
    }
}

enum ReadingOverlayService {
    private struct Reply: Decodable { var version: Int; var regions: [ReadingRegion] }
    static func load(root: URL, image: URL, cache: URL) async throws -> [ReadingRegion] {
        let process = OverlayProcess()
        return try await withTaskCancellationHandler {
            try await Task.detached(priority: .userInitiated) {
                let data = try process.run(root.appendingPathComponent("macos/.venv/bin/python"), arguments: [
                    root.appendingPathComponent("macos/backend/reading_overlay.py").path,
                    "--image", image.path, "--cache", cache.path])
                let reply = try JSONDecoder().decode(Reply.self, from: data)
                guard reply.version == 2 else { throw CocoaError(.fileReadCorruptFile) }
                return reply.regions
            }.value
        } onCancel: { process.cancel() }
    }
}
