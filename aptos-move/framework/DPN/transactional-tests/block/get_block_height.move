//# init --validators Vivian

//# run --admin-script --signers DiemRoot DiemRoot
script{
use DiemFramework::Block;
fun main() {
    // check that the height of the initial block is zero
    assert!(Block::get_current_block_height() == 0, 77);
}
}

//# block --proposer Vivian --time 100000000

//# run --admin-script --signers DiemRoot DiemRoot
script{
use DiemFramework::Block;
use DiemFramework::Timestamp;

fun main() {
    assert!(Block::get_current_block_height() == 1, 76);
    assert!(Timestamp::now_microseconds() == 100000000, 80);
}
}

//# block --proposer Vivian --time 101000000

//# run --admin-script --signers DiemRoot DiemRoot
script{
use DiemFramework::Block;
use DiemFramework::Timestamp;

fun main() {
    assert!(Block::get_current_block_height() == 2, 76);
    assert!(Timestamp::now_microseconds() == 101000000, 80);
}
}
