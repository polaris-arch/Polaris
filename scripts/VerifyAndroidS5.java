import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import javax.xml.XMLConstants;
import javax.xml.parsers.DocumentBuilderFactory;
import org.w3c.dom.Element;

class VerifyAndroidS5 {
    private static final Path APP = Path.of("src-tauri/gen/android/app");
    private static final String PACKAGE = "com.polaris2.app.vpn.";
    private static final List<String> CLASSES = List.of(
        "DefaultNetworkObservationTest", "NetworkMonitorSessionTest", "LoadedLibboxIdentityTest",
        "AndroidCallbackSchedulingSourceTest", "AndroidNativeAdmissionTest", "AndroidNativeMainControlsTest",
        "MainNativeAdmissionWiringTest", "DualModeEndpointWiringTest", "RuntimePersistenceSourceTest",
        "RuntimeProtocolAdmissionTest"
    );
    private static final Pattern TEST = Pattern.compile(
        "^\\s*@Test(?:\\([^\\n)]*\\))?\\s+fun\\s+([\\p{L}\\p{N}_]+)\\s*\\(",
        Pattern.MULTILINE | Pattern.UNICODE_CHARACTER_CLASS
    );
    private static final int MAX_REPORT = 2 * 1024 * 1024;

    static void require(boolean condition, String message) {
        if (!condition) throw new IllegalArgumentException(message);
    }

    static String utf8(byte[] raw) throws IOException {
        return StandardCharsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(raw)).toString();
    }

    static String sha256(byte[] raw) throws Exception {
        return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(raw));
    }

    static boolean named(Element element, String name) {
        return element.getNamespaceURI() == null && element.getTagName().equals(name);
    }

    static List<Element> children(Element node, String name) {
        var result = new ArrayList<Element>();
        for (var child = node.getFirstChild(); child != null; child = child.getNextSibling()) {
            if (child instanceof Element element && named(element, name)) result.add(element);
        }
        return result;
    }

    static int count(Element element, String key) {
        return Integer.parseInt(element.getAttribute(key).strip());
    }

    static List<String> sorted(List<String> strings) {
        return strings.stream()
            .sorted((left, right) -> Arrays.compare(left.codePoints().toArray(), right.codePoints().toArray()))
            .toList();
    }

    static String row(DocumentBuilderFactory parser, String name) throws Exception {
        var raw = Files.readAllBytes(APP.resolve("src/test/java/com/polaris2/app/vpn/" + name + ".kt"));
        var methods = TEST.matcher(utf8(raw)).results().map(result -> result.group(1)).toList();
        require(!methods.isEmpty() && methods.stream().distinct().count() == methods.size(),
            "Empty/duplicate S5 test source inventory: " + name);

        var classname = PACKAGE + name;
        var report = APP.resolve("build/test-results/testArm64ReleaseUnitTest/TEST-" + classname + ".xml");
        byte[] result;
        try (var stream = Files.newInputStream(report)) {
            result = stream.readNBytes(MAX_REPORT + 1);
        }
        require(result.length > 0 && result.length <= MAX_REPORT, "Missing/oversized S5 JUnit report: " + name);
        var suite = parser.newDocumentBuilder().parse(new java.io.ByteArrayInputStream(result)).getDocumentElement();
        require(named(suite, "testsuite") && suite.getAttribute("name").equals(classname), "Different S5 JUnit suite");
        var cases = children(suite, "testcase");
        require(cases.size() == methods.size() && cases.size() == count(suite, "tests"), "S5 methods missing or duplicated");
        var failures = List.of("failure", "error", "skipped");
        require(failures.stream().allMatch(key -> count(suite, key.equals("skipped") ? key : key + "s") == 0),
            "S5 JUnit failure/error/skip");
        require(sorted(cases.stream().map(test -> test.getAttribute("name")).toList()).equals(sorted(methods)),
            "S5 method results differ from frozen source");
        require(cases.stream().allMatch(test -> test.getAttribute("classname").equals(classname)
            && failures.stream().allMatch(tag -> children(test, tag).isEmpty())), "Different/failed/skipped S5 case");

        // Method names use the identifier pattern above and the other strings are fixed or hexadecimal.
        var methodList = sorted(methods).stream().map(method -> "\"" + method + "\"")
            .collect(Collectors.joining(", "));
        return "    {\"class\": \"" + classname + "\", \"tests\": " + cases.size()
            + ", \"methods\": [" + methodList + "], \"sourceSha256\": \"" + sha256(raw)
            + "\", \"reportSha256\": \"" + sha256(result) + "\"}";
    }

    static String candidate() throws Exception {
        var git = new ProcessBuilder("git", "rev-parse", "HEAD").redirectError(ProcessBuilder.Redirect.INHERIT).start();
        if (!git.waitFor(10, TimeUnit.SECONDS)) {
            git.destroyForcibly();
            throw new IOException("Timed out reading the S5 App candidate");
        }
        try (var stream = git.getInputStream()) {
            var sha = utf8(stream.readNBytes(128)).strip();
            require(git.exitValue() == 0 && sha.matches("[0-9a-f]{40}"), "Actual S5 App candidate required");
            return sha;
        }
    }

    public static void main(String[] args) throws Exception {
        require(args.length == 1, "usage: VerifyAndroidS5.java OUTPUT");
        var parser = DocumentBuilderFactory.newInstance();
        parser.setNamespaceAware(true);
        parser.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
        parser.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
        parser.setAttribute(XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");
        var rows = new ArrayList<String>();
        for (var name : CLASSES) rows.add(row(parser, name));
        var receipt = """
            {
              "schema": "polaris-s5-android-gradle-unit-results-v1",
              "candidate": "%s",
              "variant": "arm64Release",
              "classes": [
            %s
              ],
              "scope": "Real Gradle unit tests with the actual app/AAR inputs; no JNI/callback/device execution claim"
            }
            """.formatted(candidate(), String.join(",\n", rows));
        Files.writeString(Path.of(args[0]), receipt, StandardCharsets.UTF_8);
        System.out.print(receipt);
    }
}
