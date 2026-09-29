package polaris.qa;

// Standalone Java/Android app_process companion. No dependencies, identity, VPN or policy API.
// Launched only in the approved attended window; use a new private per-nonce directory.
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;
import java.util.concurrent.atomic.AtomicBoolean;

public final class MeshWitness {
    private static final List<String> PHASES = Arrays.asList(
        "wifi-positive", "policy-negative", "policy-restored", "cellular-positive", "wifi-return");
    private static final AtomicBoolean alive = new AtomicBoolean(true);
    private static String nonce;
    private static String file;
    private static long approvalExpires;
    private static volatile Socket activeTcp;
    private static Ledger tcpLedger, udpLedger;

    private static void require(boolean value, String message) {
        if (!value) throw new IllegalArgumentException(message);
    }
    private static int number(String raw, int min, int max) {
        int value = Integer.parseInt(raw);
        require(value >= min && value <= max, "integer outside QA bounds");
        return value;
    }
    private static String hash(String value) throws Exception {
        byte[] bytes = MessageDigest.getInstance("SHA-256").digest(value.getBytes(StandardCharsets.UTF_8));
        StringBuilder out = new StringBuilder();
        for (byte b : bytes) out.append(String.format(Locale.ROOT, "%02x", b & 255));
        return out.toString();
    }
    private static String challenge(String phase, String protocol, int i, String direction) {
        return "polaris-qa-v1:" + nonce + ":" + phase + ":" + direction + ":" + protocol + ":" + i + "\n";
    }
    private static String health(String protocol) { return "polaris-health-v1:" + nonce + ":" + protocol + "\n"; }
    private static String jsonStrings(Collection<String> values) {
        List<String> items = new ArrayList<>();
        for (String value : values) items.add("\"" + value + "\""); // callers use only fixed literals / SHA-256
        return "[" + String.join(",", items) + "]";
    }
    private static final class Ledger {
        final String protocol, instance, nonceHash;
        final int port;
        final Set<String> allowed = new HashSet<>();
        final SortedSet<String> seen = new TreeSet<>();
        int received;
        Ledger(String protocol, int port, int attempts) throws Exception {
            this.protocol = protocol;
            this.port = port;
            instance = hash(UUID.randomUUID().toString());
            nonceHash = hash(nonce);
            for (String phase : PHASES) for (int i = 0; i < attempts; i++)
                allowed.add(hash(challenge(phase, protocol, i, "pc-to-android")));
        }
        synchronized String accept(String payload, InetAddress remote) throws Exception {
            if (!alive.get() || System.currentTimeMillis() >= approvalExpires) return null;
            if (payload.equals(health(protocol)) && remote.isLoopbackAddress()) return payload;
            received++;
            String value = hash(payload);
            if (!allowed.contains(value) || !seen.add(value)) return null;
            return payload;
        }
        synchronized String json() {
            return "{\"instanceSha256\":\"" + instance + "\",\"nonceSha256\":\"" + nonceHash
                + "\",\"protocol\":\"" + protocol + "\",\"port\":" + port + ",\"alive\":" + alive.get()
                + ",\"received\":" + received + ",\"matched\":" + seen.size()
                + ",\"challengeSha256s\":" + jsonStrings(seen) + "}";
        }
    }
    private static synchronized void save() throws Exception {
        // Private directory is created/chmodded by operator before launch. Refuse prior counters.
        try (FileOutputStream out = new FileOutputStream(file)) {
            out.write(("{\"tcp\":" + tcpLedger.json() + ",\"udp\":" + udpLedger.json() + "}\n")
                .getBytes(StandardCharsets.UTF_8));
        }
    }
    private static String exchange(String protocol, String host, int port, String payload, int timeout) throws Exception {
        if (protocol.equals("tcp")) {
            try (Socket socket = new Socket()) {
                socket.connect(new InetSocketAddress(host, port), timeout);
                socket.setSoTimeout(timeout);
                socket.getOutputStream().write(payload.getBytes(StandardCharsets.UTF_8));
                socket.shutdownOutput();
                ByteArrayOutputStream out = new ByteArrayOutputStream();
                byte[] buffer = new byte[1024];
                for (int n; (n = socket.getInputStream().read(buffer)) != -1;) {
                    out.write(buffer, 0, n);
                    require(out.size() <= 1024, "oversize reply");
                }
                return out.toString("UTF-8");
            }
        }
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(InetAddress.getByName(host), port);
            socket.setSoTimeout(timeout);
            byte[] bytes = payload.getBytes(StandardCharsets.UTF_8);
            socket.send(new DatagramPacket(bytes, bytes.length));
            DatagramPacket reply = new DatagramPacket(new byte[1024], 1024);
            socket.receive(reply);
            return new String(reply.getData(), reply.getOffset(), reply.getLength(), StandardCharsets.UTF_8);
        }
    }
    private static String outcome(Exception error) {
        if (error instanceof SocketTimeoutException) return "timeout";
        if (error instanceof PortUnreachableException || error instanceof ConnectException) return "refused";
        return "local-error"; // route/bind/permission errors are never policy PASS
    }
    public static void main(String[] args) {
        try { run(args); }
        catch (Exception error) { System.err.println("FAIL " + error.getClass().getSimpleName()); System.exit(1); }
    }
    private static void run(String[] args) throws Exception {
        require(args.length >= 5, "usage requires mode, nonce and explicit execution interlock");
        // This is an operator interlock, not authentication. The Node gate validates scope/time/plan
        // before constructing this command; no secret or credential is passed into app_process.
        if (!args[args.length - 3].equals("--execute")) { System.out.println("DRY_RUN"); return; }
        require(args[args.length - 2].matches("[a-f0-9]{64}"), "approved plan digest required");
        approvalExpires = Long.parseLong(args[args.length - 1]);
        require(System.currentTimeMillis() < approvalExpires && approvalExpires - System.currentTimeMillis() <= 3600000L, "approval expired or oversized");
        nonce = args[1];
        require(nonce.matches("[a-f0-9]{48}"), "fresh 192-bit nonce required");
        String mode = args[0];
        if (mode.equals("probe")) {
            require(args.length == 12, "probe arguments");
            String phase = args[2], protocol = args[3], host = args[4];
            require(PHASES.contains(phase) && Arrays.asList("tcp", "udp").contains(protocol), "phase/protocol");
            require(host.matches("[0-9.]+") && host.equals(InetAddress.getByName(host).getHostAddress()), "IPv4 literal");
            int port = number(args[5], 1024, 65535), attempts = number(args[6], 2, 20), timeout = number(args[7], 500, 10000);
            require(args[8].equals("android-to-pc"), "direction");
            List<String> outcomes = new ArrayList<>(), acks = new ArrayList<>();
            for (int i = 0; i < attempts; i++) {
                require(System.currentTimeMillis() < approvalExpires, "approval expired");
                String payload = challenge(phase, protocol, i, "android-to-pc");
                try {
                    String reply = exchange(protocol, host, port, payload, timeout);
                    if (reply.equals(payload)) { outcomes.add("echo"); acks.add(hash(reply)); }
                    else outcomes.add("local-error");
                } catch (Exception error) { outcomes.add(outcome(error)); }
                if (outcomes.get(outcomes.size() - 1).equals("local-error")) break;
            }
            System.out.println("{\"phase\":\"" + phase + "\",\"direction\":\"android-to-pc\",\"protocol\":\"" + protocol
                + "\",\"attempted\":" + outcomes.size() + ",\"outcomes\":" + jsonStrings(outcomes) + ",\"ackSha256s\":" + jsonStrings(acks) + "}");
            return;
        }
        if (mode.equals("health")) {
            require(args.length == 8, "health arguments");
            String protocol = args[2];
            require(Arrays.asList("tcp", "udp").contains(protocol), "protocol");
            String payload = health(protocol);
            require(exchange(protocol, "127.0.0.1", number(args[3], 1024, 65535), payload,
                number(args[4], 500, 10000)).equals(payload), "loopback echo failed");
            System.out.println("{\"loopbackHealth\":true}");
            return;
        }
        require(mode.equals("serve") && args.length == 10, "serve arguments");
        int tcpPort = number(args[2], 1024, 65535), udpPort = number(args[3], 1024, 65535);
        int attempts = number(args[4], 2, 20), seconds = number(args[6], 1, 900);
        file = args[5];
        require(file.equals("cache/polaris-qa-" + nonce + "/counters.json"), "private per-nonce app counter path required");
        require(!new File(file).exists(), "prior counter file forbidden");
        tcpLedger = new Ledger("tcp", tcpPort, attempts);
        udpLedger = new Ledger("udp", udpPort, attempts);
        InetAddress loopback = InetAddress.getByName("127.0.0.1");
        final ServerSocket tcp = new ServerSocket(tcpPort, 8, loopback);
        final DatagramSocket udp;
        try { udp = new DatagramSocket(new InetSocketAddress(loopback, udpPort)); }
        catch (Exception error) { tcp.close(); throw error; }
        Runnable stop = () -> {
            alive.set(false);
            Socket active = activeTcp;
            if (active != null) try { active.close(); } catch (Exception ignored) {}
            try { tcp.close(); } catch (Exception ignored) {}
            udp.close();
            try { save(); } catch (Exception ignored) { System.err.println("FAIL counter-save"); }
        };
        Runtime.getRuntime().addShutdownHook(new Thread(stop));
        Thread tcpThread = new Thread(() -> {
            try {
                while (alive.get()) try (Socket socket = tcp.accept()) {
                    activeTcp = socket;
                    if (!alive.get()) break;
                    socket.setSoTimeout(10000);
                    ByteArrayOutputStream out = new ByteArrayOutputStream();
                    byte[] buffer = new byte[1024];
                    for (int n; (n = socket.getInputStream().read(buffer)) != -1;) {
                        out.write(buffer, 0, n);
                        require(out.size() <= 1024, "oversize request");
                    }
                    String reply = tcpLedger.accept(out.toString("UTF-8"), socket.getInetAddress());
                    save();
                    if (reply != null) socket.getOutputStream().write(reply.getBytes(StandardCharsets.UTF_8));
                    activeTcp = null;
                }
            } catch (Exception error) { if (alive.get()) System.err.println("FAIL tcp-listener"); stop.run(); }
        });
        Thread udpThread = new Thread(() -> {
            try {
                while (alive.get()) {
                    DatagramPacket packet = new DatagramPacket(new byte[1024], 1024);
                    udp.receive(packet);
                    String reply = udpLedger.accept(new String(packet.getData(), packet.getOffset(), packet.getLength(), StandardCharsets.UTF_8), packet.getAddress());
                    save();
                    if (reply != null) { byte[] bytes = reply.getBytes(StandardCharsets.UTF_8); udp.send(new DatagramPacket(bytes, bytes.length, packet.getAddress(), packet.getPort())); }
                }
            } catch (Exception error) { if (alive.get()) System.err.println("FAIL udp-listener"); stop.run(); }
        });
        save();
        tcpThread.start(); udpThread.start();
        System.out.println("{\"state\":\"READY\",\"planSha256\":\"" + args[8] + "\"}");
        long end = Math.min(approvalExpires, System.currentTimeMillis() + seconds * 1000L);
        while (alive.get() && System.currentTimeMillis() < end) Thread.sleep(100);
        stop.run(); tcpThread.join(); udpThread.join();
    }
}
