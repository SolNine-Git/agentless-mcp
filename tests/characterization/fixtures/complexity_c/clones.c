#include <stdio.h>

int scaled_sum(int left, int right) {
    int total = left + right;
    printf("%d\n", total);
    return total * 2;
}

int doubled_total(int a, int b) {
    int sum = a + b;
    printf("sum %d\n", sum);
    return sum * 3;
}

int near_miss(int left, int right) {
    int total = left - right;
    printf("%d\n", total);
    return total * 2;
}
