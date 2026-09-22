// Perry 嵌套调用参数绑定最小复现
// 预期（node / 正确语义）：
//   L3 outer(mid()): id=42   <- 三层嵌套
//   L2 outer(inner()): id=42 <- 两层嵌套
//   bind outer(m): id=42     <- 先绑定局部变量
function inner(): { id: number } {
    return { id: 42 };
}
function mid(): { id: number } {
    return inner();
}
function outer(x: { id: number } | null): void {
    if (x === null) {
        console.log("outer got null");
        return;
    }
    console.log("outer got id =", x.id);
}
outer(mid());
outer(inner());
const m = mid();
outer(m);
