// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibTeamStorage} from "../libraries/LibTeamStorage.sol";

/// @title TeamFacet
/// @notice Agent TEAMS — the mutual-consent membership layer over the WebRTC
///         P2P collaboration transport. Agents form teams by AGREEMENT: a
///         member `invite`s an agent, and that agent must `accept` (no one is
///         added without their own signature). A team then becomes a signaling
///         topic — `keccak256("team", teamId)` — that members `announce` under
///         and sync within (`SignalingFacet` + WebRTC). This makes P2P sync,
///         x402 payment, and `call_agent` all flow along the SAME consented
///         peer set: a team is the unit of agent collaboration.
///
///         "Sync my own devices" is just the degenerate team (you + you), which
///         needs no invite/accept — it uses the `keccak256("devices", owner)`
///         topic directly. Multi-agent teams use this facet.
///
///         CUTTING IT (diamond owner; mirror script/AddTbaFacet.s.sol):
///         deploy + diamondCut Add [createTeam(string), invite(uint256,address),
///         accept(uint256), decline(uint256), leave(uint256),
///         membersOf(uint256), teamsOf(address), isMember(uint256,address),
///         isInvited(uint256,address), teamName(uint256), nextTeamId()].
contract TeamFacet {
    event TeamCreated(uint256 indexed teamId, address indexed creator, string name);
    event Invited(uint256 indexed teamId, address indexed agent, address indexed by);
    event Joined(uint256 indexed teamId, address indexed agent);
    event Left(uint256 indexed teamId, address indexed agent);

    error NotMember();
    error NotInvited();

    /// Create a team; the caller is its first member. Returns the new id.
    function createTeam(string calldata name) external returns (uint256 teamId) {
        LibTeamStorage.Storage storage s = LibTeamStorage.load();
        teamId = ++s.nextTeamId; // ids start at 1
        s.teams[teamId].name = name;
        _addMember(s, teamId, msg.sender);
        emit TeamCreated(teamId, msg.sender, name);
    }

    /// Member-only: invite an agent. They must `accept` (the consent half).
    function invite(uint256 teamId, address agent) external {
        LibTeamStorage.Storage storage s = LibTeamStorage.load();
        if (!s.isMember[teamId][msg.sender]) revert NotMember();
        s.invited[teamId][agent] = true;
        emit Invited(teamId, agent, msg.sender);
    }

    /// Invitee-only: accept a pending invite → become a member. BOTH sides have
    /// now agreed (a member invited, you accepted).
    function accept(uint256 teamId) external {
        LibTeamStorage.Storage storage s = LibTeamStorage.load();
        if (!s.invited[teamId][msg.sender]) revert NotInvited();
        s.invited[teamId][msg.sender] = false;
        _addMember(s, teamId, msg.sender);
        emit Joined(teamId, msg.sender);
    }

    /// Invitee-only: decline a pending invite without joining.
    function decline(uint256 teamId) external {
        LibTeamStorage.load().invited[teamId][msg.sender] = false;
    }

    /// Leave a team you're in.
    function leave(uint256 teamId) external {
        _removeMember(LibTeamStorage.load(), teamId, msg.sender);
        emit Left(teamId, msg.sender);
    }

    function membersOf(uint256 teamId) external view returns (address[] memory) {
        return LibTeamStorage.load().teams[teamId].members;
    }

    function teamsOf(address agent) external view returns (uint256[] memory) {
        return LibTeamStorage.load().teamsOf[agent];
    }

    function isMember(uint256 teamId, address agent) external view returns (bool) {
        return LibTeamStorage.load().isMember[teamId][agent];
    }

    function isInvited(uint256 teamId, address agent) external view returns (bool) {
        return LibTeamStorage.load().invited[teamId][agent];
    }

    function teamName(uint256 teamId) external view returns (string memory) {
        return LibTeamStorage.load().teams[teamId].name;
    }

    function nextTeamId() external view returns (uint256) {
        return LibTeamStorage.load().nextTeamId;
    }

    // --- internal -------------------------------------------------------

    function _addMember(LibTeamStorage.Storage storage s, uint256 teamId, address agent) internal {
        if (s.isMember[teamId][agent]) return; // idempotent
        s.isMember[teamId][agent] = true;
        s.teams[teamId].members.push(agent);
        s.memberIndex[teamId][agent] = s.teams[teamId].members.length; // index + 1
        s.teamsOf[agent].push(teamId);
    }

    function _removeMember(LibTeamStorage.Storage storage s, uint256 teamId, address agent)
        internal
    {
        uint256 idx1 = s.memberIndex[teamId][agent];
        if (idx1 == 0) return; // not a member
        // swap-pop the members array
        address[] storage m = s.teams[teamId].members;
        uint256 i = idx1 - 1;
        uint256 last = m.length - 1;
        if (i != last) {
            address moved = m[last];
            m[i] = moved;
            s.memberIndex[teamId][moved] = i + 1;
        }
        m.pop();
        s.memberIndex[teamId][agent] = 0;
        s.isMember[teamId][agent] = false;
        // swap-pop the agent's teamsOf
        uint256[] storage ts = s.teamsOf[agent];
        for (uint256 j = 0; j < ts.length; j++) {
            if (ts[j] == teamId) {
                ts[j] = ts[ts.length - 1];
                ts.pop();
                break;
            }
        }
    }
}
