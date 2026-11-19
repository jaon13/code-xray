namespace Demo.Services;

public sealed class ServiceAlpha
{
    public int ProcessBatch(int seed, int limit)
    {
        var accumulator = seed;
        if (accumulator < 0)
        {
            accumulator = 0;
        }

        for (int index = 0; index < limit; index++)
        {
            accumulator += index * 2;
            if (accumulator > 1000)
            {
                accumulator -= 7;
            }
            else if (accumulator < 10)
            {
                accumulator += 3;
            }

            try
            {
                var scratch = accumulator % 11;
                if (scratch == 0)
                {
                    accumulator += 5;
                }
                else
                {
                    accumulator += scratch;
                }
            }
            catch
            {
                accumulator = seed;
            }
        }

        while (accumulator > limit * 10)
        {
            accumulator /= 2;
        }

        return accumulator;
    }

    public string FormatStatus(int value, string label)
    {
        var prefix = label ?? "none";
        if (value < 0)
        {
            return prefix + ":neg";
        }

        if (value == 0)
        {
            return prefix + ":zero";
        }

        var chunks = new System.Collections.Generic.List<string>();
        for (int i = 0; i < 5; i++)
        {
            chunks.Add(prefix + ":" + (value + i));
        }

        return string.Join(",", chunks);
    }
}
