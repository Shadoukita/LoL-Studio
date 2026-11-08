-- The functions of Summoner's Rift's NeutralMinionSpawn, rewritten from its bytecode.

function CreateRespawnNeutralTimer(campIndex)
	return function()
		local roll = math.random(100)
		local chance = 0
		local spawned = false
		local c = NeutralMinionCamps[campIndex]
		c.DamageBonus = c.DamageBonus + 0
		c.HealthBonus = c.HealthBonus + 0
		for g = 1, table.getn(c.Groups) do
			if roll <= c.GroupsChance[g] + chance and spawned == false then
				spawned = true
				c.AliveTracker = {}
				for m = 1, table.getn(c.Groups[g]) do
					SpawnNeutralMinion(c, campIndex, g, m)
					c.AliveTracker[m] = true
				end
			end
			chance = chance + c.GroupsChance[g]
		end
	end
end

function InitializeNeutralMinionInfo()
	for i = 1, table.getn(NeutralMinionCamps) do
		local c = NeutralMinionCamps[i]
		for g = 1, table.getn(c.Groups) do
			c.UniqueNames[g] = {}
			for m = 1, table.getn(c.Groups[g]) do
				c.UniqueNames[g][m] = c.Groups[g][m] .. i .. "." .. g .. "." .. m
			end
		end
		local t = table.getn(NeutralTimers)
		NeutralTimers[t + 1] = CreateRespawnNeutralTimer(i)
		c.Timer = NeutralTimers[t + 1]
		CreateNeutralCamp(c, i)
	end
end

function SpawnInitialNeutralMinions(campIndex)
	if campIndex <= table.getn(NeutralMinionCamps) then
		local roll = math.random(100)
		local chance = 0
		local spawned = false
		local c = NeutralMinionCamps[campIndex]
		for g = 1, table.getn(c.Groups) do
			if 0 < c.GroupDelaySpawnTime then
				if spawned == false then
					spawned = true
					c.AliveTracker = {}
					for m = 1, table.getn(c.Groups[g]) do
						c.AliveTracker[m] = true
					end
				end
			elseif roll <= c.GroupsChance[g] + chance and spawned == false then
				spawned = true
				c.AliveTracker = {}
				for m = 1, table.getn(c.Groups[g]) do
					c.AliveTracker[m] = true
				end
			end
			chance = chance + c.GroupsChance[g]
		end
	end
end

function NeutralMinionDeath(name, killer, campHint)
	local allDead = true
	local camp = 0
	for i = 1, table.getn(NeutralMinionCamps) do
		local c = NeutralMinionCamps[i]
		for g = 1, table.getn(c.UniqueNames) do
			for m = 1, table.getn(c.UniqueNames[g]) do
				if c.UniqueNames[g][m] == name then
					camp = i
					c.AliveTracker[m] = false
				end
			end
		end
	end
	if 0 < camp then
		local c = NeutralMinionCamps[camp]
		for m = 1, table.getn(c.AliveTracker) do
			if c.AliveTracker[m] == true then
				allDead = false
			end
		end
		if allDead == true then
			InitNeutralMinionTimer(c.Timer, c.TimerType, c.GroupsRespawnTime, 0, false)
		end
	end
end
