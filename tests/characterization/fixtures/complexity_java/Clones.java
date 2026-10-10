class Clones {
    int scaledSum(int left, int right) {
        int total = left + right;
        System.out.println("sum");
        return total * 2;
    }

    int doubledTotal(int a, int b) {
        int sum = a + b;
        System.out.println("total");
        return sum * 3;
    }

    int nearMiss(int left, int right) {
        int total = left - right;
        System.out.println("sum");
        return total * 2;
    }
}
