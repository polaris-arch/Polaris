package polaris.qa;

// Prepared-only Android artifact verifier. No sockets, echo server, VPN or policy API.
// Compile against the pinned Android platform jar; execute only under reviewed Debug run-as.
import android.os.Process;
import android.system.Os;
import android.system.StructStat;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;
import org.json.JSONObject;

public final class MeshWitness {
    private static void require(boolean value, String message) {
        if (!value) throw new IllegalArgumentException(message);
    }
    private static String hash(byte[] value) throws Exception {
        byte[] bytes = MessageDigest.getInstance("SHA-256").digest(value);
        StringBuilder out = new StringBuilder();
        for (byte b : bytes) out.append(String.format(Locale.ROOT, "%02x", b & 255));
        return out.toString();
    }
    private static byte[] read(File file) throws Exception {
        require(file.isFile(), "artifact file missing");
        try (FileInputStream in = new FileInputStream(file); ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[4096];
            for (int n; (n = in.read(buffer)) != -1;) {
                out.write(buffer, 0, n);
                require(out.size() <= 4 * 1024 * 1024, "artifact exceeds verifier limit");
            }
            return out.toByteArray();
        }
    }
    private static File privateFile(String nonce, String supplied, String name) throws Exception {
        String expected = "cache/polaris-qa-" + nonce + "/" + name;
        require(supplied.equals(expected), "artifact must use exact nonce-owned private path");
        File app = new File(".").getCanonicalFile();
        require(app.getPath().matches("/data/(?:user/[0-9]+|data)/com\\.polaris2\\.app\\.debug"), "Debug app data directory required");
        File root = new File(app, "cache/polaris-qa-" + nonce);
        require(root.getCanonicalPath().equals(root.getAbsolutePath()), "symlink QA root forbidden");
        StructStat rootStat = Os.lstat(root.getPath());
        require((rootStat.st_mode & 0777) == 0700 && rootStat.st_uid == Process.myUid(), "app-owned 0700 QA root required");
        File file = new File(app, expected);
        require(file.getCanonicalPath().equals(file.getAbsolutePath()), "symlink artifact forbidden");
        StructStat stat = Os.lstat(file.getPath());
        require((stat.st_mode & 0222) == 0 && stat.st_uid == Process.myUid(), "app-owned read-only artifact required");
        return file;
    }
    private static String canonicalBuild(JSONObject build) throws Exception {
        TreeSet<String> keys = new TreeSet<>();
        Iterator<String> iterator = build.keys();
        while (iterator.hasNext()) keys.add(iterator.next());
        require(keys.equals(new TreeSet<>(Arrays.asList("schemaVersion", "sourceSha256", "javacSha256",
            "d8Sha256", "androidJarSha256", "classJarSha256", "dexSha256", "minApi"))), "build receipt shape mismatch");
        require(build.getInt("schemaVersion") == 1 && build.getInt("minApi") == 24, "build receipt version mismatch");
        StringBuilder out = new StringBuilder("{");
        for (String key : keys) {
            if (out.length() > 1) out.append(',');
            out.append('"').append(key).append("\":");
            if (key.endsWith("Sha256")) {
                String value = build.getString(key);
                require(value.matches("[a-f0-9]{64}"), "build hash mismatch");
                out.append('"').append(value).append('"');
            } else out.append(build.getInt(key));
        }
        return out.append('}').toString();
    }
    public static void main(String[] args) {
        try {
            if (Arrays.asList(args).contains("--execute")) {
                throw new IllegalArgumentException("NOT_READY: prepared-only; execution disabled");
            }
            if (args.length == 0) {
                System.out.println("{\"mode\":\"DRY_RUN\",\"verdict\":\"NOT_READY\",\"claims\":[]}");
                return;
            }
            require(args.length == 5 && args[0].equals("verify"), "only no-socket private-artifact verification is implemented");
            String nonce = args[1], planDigest = args[2];
            require(nonce.matches("[a-f0-9]{48}") && planDigest.matches("[a-f0-9]{64}"), "nonce/plan hash required");
            byte[] manifestBytes = read(privateFile(nonce, args[3], "manifest.json"));
            require(hash(manifestBytes).equals(planDigest), "private canonical plan digest mismatch");
            JSONObject plan = new JSONObject(new String(manifestBytes, StandardCharsets.UTF_8));
            require(plan.getString("nonce").equals(nonce), "private manifest nonce mismatch");
            require(plan.getString("package").equals("com.polaris2.app.debug"), "private manifest package mismatch");
            JSONObject readiness = plan.getJSONObject("readiness");
            require(readiness.getString("level").equals("prepared-only") && !readiness.getBoolean("executable"), "unrecognized readiness profile");
            JSONObject build = plan.getJSONObject("witnessBuild");
            String buildDigest = hash(canonicalBuild(build).getBytes(StandardCharsets.UTF_8));
            require(buildDigest.equals(plan.getString("witnessBuildReceiptSha256")), "private build receipt digest mismatch");
            String dexDigest = hash(read(privateFile(nonce, args[4], "witness.dex.jar")));
            require(dexDigest.equals(build.getString("dexSha256")), "private witness jar digest mismatch");
            System.out.println("{\"mode\":\"PRIVATE_ARTIFACT_VERIFIED\",\"verdict\":\"NOT_READY\",\"claims\":[],"
                + "\"manifestSha256\":\"" + planDigest + "\",\"dexSha256\":\"" + dexDigest
                + "\",\"buildReceiptSha256\":\"" + buildDigest + "\",\"nonceSha256\":\""
                + hash(nonce.getBytes(StandardCharsets.UTF_8)) + "\"}");
        } catch (Exception error) {
            System.err.println("FAIL " + error.getClass().getSimpleName());
            System.exit(1);
        }
    }
}
