#include <string>

int scaled_sum(int left, int right) {
    int total = left + right;
    std::string label = "sum";
    return total * 2;
}

int doubled_total(int a, int b) {
    int sum = a + b;
    std::string name = "total";
    return sum * 3;
}

int near_miss(int left, int right) {
    int total = left - right;
    std::string label = "sum";
    return total * 2;
}
