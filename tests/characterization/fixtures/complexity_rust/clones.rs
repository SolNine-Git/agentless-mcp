fn scaled_sum(left: i32, right: i32) -> i32 {
    let total = left + right;
    println!("sum {}", total);
    total * 2
}

fn doubled_total(a: i32, b: i32) -> i32 {
    let sum = a + b;
    println!("total {}", sum);
    sum * 3
}

fn near_miss(left: i32, right: i32) -> i32 {
    let total = left - right;
    println!("sum {}", total);
    total * 2
}
