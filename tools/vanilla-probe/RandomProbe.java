import java.util.Random;

public class RandomProbe {
    public static void main(String[] args) {
        int[] seeds = {42, 0, 12345, 7, 99, 5, 11, 3, 1, 2024};
        for (int seed : seeds) {
            Random r = new Random(seed);
            StringBuilder sb = new StringBuilder();
            sb.append("seed=").append(seed).append(" nextInt=").append(r.nextInt());
            sb.append(" nextInt=").append(r.nextInt());
            sb.append(" nextLong=").append(r.nextLong());
            sb.append(" nextDouble=").append(r.nextDouble());
            sb.append(" nextFloat=").append(r.nextFloat());
            sb.append(" nextBoolean=").append(r.nextBoolean());
            System.out.println(sb);
        }
        // Bounded draws, including the power-of-two branch.
        Random bounded = new Random(7);
        StringBuilder sb = new StringBuilder("seed=7 bounded:");
        for (int i = 0; i < 8; i++) {
            sb.append(' ').append(bounded.nextInt(37));
        }
        System.out.println(sb);
        Random pow2 = new Random(7);
        StringBuilder sb2 = new StringBuilder("seed=7 pow2:");
        for (int i = 0; i < 8; i++) {
            sb2.append(' ').append(pow2.nextInt(64));
        }
        System.out.println(sb2);
        Random dbl = new Random(5);
        StringBuilder sb3 = new StringBuilder("seed=5 doubles:");
        for (int i = 0; i < 5; i++) {
            sb3.append(' ').append(dbl.nextDouble());
        }
        System.out.println(sb3);
        Random flt = new Random(5);
        StringBuilder sb4 = new StringBuilder("seed=5 floats:");
        for (int i = 0; i < 5; i++) {
            sb4.append(' ').append(flt.nextFloat());
        }
        System.out.println(sb4);
    }
}
