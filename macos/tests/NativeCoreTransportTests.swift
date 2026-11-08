import Foundation

@main struct NativeCoreTransportTests {
    @MainActor static func main() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let output = URL(fileURLWithPath: CommandLine.arguments[2])
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let fake = output.appendingPathComponent("peer.py")
        try #"""
import json, sys, threading, time, os
from pathlib import Path
assert 'KOMA_TRANSLATION_API_KEY' not in os.environ
lock=threading.Lock(); cancelled={}
def send(value):
    with lock:
        sys.stdout.write(json.dumps(value)+'\n');sys.stdout.flush()
def run(r):
    ident=r['id'];op=r['op']
    if op=='slow':
        send({'id':ident,'event':'progress','message':'started'})
        cancelled[ident].wait(10)
        send({'id':ident,'event':'cancelled'})
    elif op=='large':send({'id':ident,'event':'result','result':{'text':'中'*3000000}})
    elif op=='error':send({'id':ident,'event':'error','error':'expected error'})
    else:send({'id':ident,'event':'result','result':{'echo':r.get('value',0)}})
for line in sys.stdin:
    r=json.loads(line)
    if r['op']=='cancel':
        cancelled.get(r['target'],threading.Event()).set()
        send({'id':r['id'],'event':'result','result':{}})
    else:
        cancelled[r['id']]=threading.Event()
        threading.Thread(target=run,args=(r,),daemon=True).start()
Path(__file__).with_suffix('.closed').write_text('closed')
"""#.write(to: fake, atomically: true, encoding: .utf8)
        let service = NativeCoreService(executable: root.appendingPathComponent("macos/.venv/bin/python"),
                                        arguments: [fake.path], logURL: output.appendingPathComponent("transport.log"))
        func object(_ data: Data) throws -> [String: Any] { try JSONSerialization.jsonObject(with: data) as! [String: Any] }
        // Concurrent responses must match their request IDs, even while a large
        // reply fills multiple pipe reads and another job is awaiting cancel.
        var started = false
        let slow = Task { try await service.request("slow", progress: { _ in started = true }) }
        while !started { try await Task.sleep(for: .milliseconds(10)) }
        let echoes = (0..<20).map { value in Task { @MainActor in
            let result = try object(await service.request("echo", values: ["value": value]))
            precondition(result["echo"] as? Int == value)
        } }
        for echo in echoes { try await echo.value }
        let large = try object(await service.request("large"))
        precondition((large["text"] as? String)?.count == 3_000_000)
        slow.cancel()
        do { _ = try await slow.value; preconditionFailure("cancel did not throw") }
        catch is CancellationError { }
        do { _ = try await service.request("error"); preconditionFailure("error did not throw") }
        catch let error as NativeCoreService.Failure { precondition(error.message == "expected error") }
        let again = try object(await service.request("echo", values: ["value": 99]))
        precondition(again["echo"] as? Int == 99)
        service.shutdown()
        let closed = fake.deletingPathExtension().appendingPathExtension("closed")
        for _ in 0..<200 {
            if FileManager.default.fileExists(atPath: closed.path) { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        precondition(FileManager.default.fileExists(atPath: closed.path))

        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent(".runtime/rust-migration/pages.json"))) as! [[String: Any]]
        let seed = URL(fileURLWithPath: manifest[0]["cache"] as! String).deletingLastPathComponent().deletingLastPathComponent()
        let native = NativeCoreService(root: root, cache: output.appendingPathComponent("pages"),
                                       assets: output.appendingPathComponent("assets"), seed: seed)
        let health = try object(await native.request("health"))
        precondition(health["protocol"] as? Int == 1)
        for page in manifest {
            let loaded = try object(await native.request("load_page", values: ["image":page["image"]!, "mode":"vision"]))
            let copy = try Data(contentsOf: URL(fileURLWithPath: loaded["cache"] as! String))
            let original = try Data(contentsOf: URL(fileURLWithPath: page["cache"] as! String))
            precondition(copy == original)
        }
        let result = try object(await native.request("process", values: ["images":manifest.map {$0["image"]!},
            "mode":"vision", "operation":"ocr", "model":"unused"]))
        let pages = result["pages"] as! [[String: Any]]
        precondition(pages.count == 35 && pages.allSatisfy {$0["ok"] as? Bool == true})
        native.shutdown()
        print("Native Swift transport passed: multiplexing, large reply, cancellation, error recovery, EOF, 35-page real Rust queue.")
    }
}
