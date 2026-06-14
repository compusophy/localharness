// Art — a minimal ownable/tradable ERC-721-style NFT collection, written in
// SolidityLite (the in-browser Solidity/EVM-subset compiler). An agent compiles +
// deploys + cuts this into its OWN child diamond, giving it a self-sovereign art
// collection that humans or other agents can collect via `transfer`:
//
//   localharness facet deploy art templates/art.sol          # compile + deploy
//   localharness facet diamond                               # your child diamond
//   localharness facet cut <your-diamond> <art-addr> templates/art.sol
//
// Standard selectors (name 0x06fdde03, symbol 0x95d89b41, mint 0x1249c58b,
// transfer 0xa9059cbb, ownerOf 0x6352211e, balanceOf 0x70a08231,
// totalSupply 0x18160ddd). Proven live on Tempo Moderato (mint → transfer →
// ownerOf/balanceOf). The whole file is in the v1 SolidityLite subset: value-type
// mappings, scalars, msg.sender, require, `+`/`-`, indexed events, and constant
// `string` returns.
facet Art {
    mapping(uint256 => address) owners;   // tokenId  => current owner
    mapping(address => uint256) balances;  // owner    => token count
    uint256 nextId;                        // next tokenId to mint

    event Transfer(address indexed from, address indexed to, uint256 indexed id);

    function name() external pure returns (string) { return "Localharness Art"; }
    function symbol() external pure returns (string) { return "LART"; }

    function mint() external {
        owners[nextId] = msg.sender;
        balances[msg.sender] = balances[msg.sender] + 1;
        emit Transfer(0, msg.sender, nextId);   // mint = transfer from address(0)
        nextId = nextId + 1;
    }

    function transfer(address to, uint256 id) external {
        require(owners[id] == msg.sender, "not owner");
        owners[id] = to;
        balances[msg.sender] = balances[msg.sender] - 1;
        balances[to] = balances[to] + 1;
        emit Transfer(msg.sender, to, id);
    }

    function ownerOf(uint256 id) external view returns (address) { return owners[id]; }
    function balanceOf(address who) external view returns (uint256) { return balances[who]; }
    function totalSupply() external view returns (uint256) { return nextId; }
}
