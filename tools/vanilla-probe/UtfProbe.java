import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;

public class UtfProbe {
    public static void main(String[] args) throws Exception {
        for (String s : new String[] { "\u0000", "\u00e9", "\uD83D\uDE00", "a\u0000b" }) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            DataOutputStream out = new DataOutputStream(bytes);
            out.writeUTF(s);
            out.flush();
            StringBuilder hex = new StringBuilder();
            for (byte b : bytes.toByteArray()) {
                hex.append(String.format("%02X ", b));
            }
            System.out.println("writeUTF(" + escape(s) + ") = " + hex.toString().trim());
        }
    }

    static String escape(String s) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < s.length(); i++) {
            sb.append(String.format("\\u%04X", (int) s.charAt(i)));
        }
        return sb.toString();
    }
}
